//! Rust owns the engine's block tail: the signal that leaves the track graph.
//!
//! The C++ callback used to capture feedback, publish metering, render the
//! metronome, scale and limit the master, measure it, mix the Control Room cue
//! and advance the playhead -- 140 lines of realtime policy inlined in
//! `processBlock`, plus its click scratch and the double-buffered telemetry
//! handoff. The engine binds its handles once into this state and calls
//! [`hirari_engine_block_finish`] with the block the graph just filled.
//!
//! Tracks stay C++ objects, so the three things this state cannot reach through
//! a handle -- a track's own runtime, whether it is a bus, and its armed
//! recorder -- travel as a published operations table.

use crate::bounce_driver::hirari_bounce_offline_active;
use crate::control_room_audio::hirari_control_room_mix_cue;
use crate::control_room_runtime::{
    hirari_control_room_active_cue_bus, hirari_control_room_active_cue_click,
    hirari_control_room_active_cue_gain, hirari_control_room_active_cue_output,
    hirari_control_room_process_monitor_state,
};
use crate::engine_input_block::hirari_input_blocks_take_talkback;
use crate::master_limiter::hirari_master_limiter_process;
use crate::master_meter::hirari_master_meter_process;
use crate::master_output::hirari_master_output_process;
use crate::metronome_renderer::{hirari_metronome_is_enabled, hirari_metronome_process};
use crate::mixer_telemetry::hirari_mixer_track_peaks;
use crate::routing_engine_core::hirari_routing_engine_capture_feedback;
use crate::tempo::hirari_tempo_map_get_current_bpm;
use crate::track_runtime::{self, TrackRuntime};
use std::ffi::c_void;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// `HirariUnifiedEngine::kMaxTracks`, and the width of one metering payload.
const MAX_TRACKS: usize = 256;
/// `HirariUnifiedEngine::TelemetryData::spectrum` bins.
const SPECTRUM_BINS: usize = 512;
/// `HirariUnifiedEngine::kMaxAudioBlockSize`; the click scratch holds one block.
const MAX_BLOCK_FRAMES: usize = 16_384;

/// Mirrors `HirariUnifiedEngine::TelemetryData` field for field. The audio
/// callback writes only `count`, the two peak arrays and `version`; every other
/// field stays at its published default, exactly as it did in C++.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariEngineTelemetry {
    pub peaks_left: [f32; MAX_TRACKS],
    pub peaks_right: [f32; MAX_TRACKS],
    pub count: u32,
    pub spectrum: [f32; SPECTRUM_BINS],
    pub correlation: f32,
    pub dsp_load: f32,
    pub active_voices: u32,
    pub forensic_audit_hash: u64,
    pub version: u64,
}

impl Default for HirariEngineTelemetry {
    fn default() -> Self {
        Self {
            peaks_left: [0.0; MAX_TRACKS],
            peaks_right: [0.0; MAX_TRACKS],
            count: 0,
            spectrum: [0.0; SPECTRUM_BINS],
            correlation: 0.0,
            dsp_load: 0.0,
            active_voices: 0,
            forensic_audit_hash: 0,
            version: 0,
        }
    }
}

/// The per-track operations the tail needs from C++.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariEngineBlockOps {
    /// The track's bound [`TrackRuntime`] handle.
    pub runtime: Option<unsafe extern "C" fn(*mut c_void) -> *mut c_void>,
    /// Non-zero when the track is an AUX/BUS track.
    pub is_bus: Option<unsafe extern "C" fn(*mut c_void) -> u8>,
    /// Captures the fader and pan of one armed recorder.
    pub record: Option<unsafe extern "C" fn(*mut c_void, u64)>,
}

/// The graph window the callback is walking, exactly as the engine's head
/// resolved it. A hot-swap can publish a different table mid-block, so the tail
/// never re-reads it.
#[derive(Clone, Copy, Default)]
struct View {
    tracks: *const *mut c_void,
    count: u32,
}

impl View {
    fn active(self) -> &'static [*mut c_void] {
        unsafe {
            slice::from_raw_parts(
                self.tracks,
                (self.count as usize).min(MAX_TRACKS),
            )
        }
    }
}

pub struct EngineBlockState {
    ops: HirariEngineBlockOps,
    routing: *mut c_void,
    bounce: *mut c_void,
    input_blocks: *mut c_void,
    control_room: *mut c_void,
    mixer_telemetry: *mut c_void,
    metronome: *mut c_void,
    master_limiter: *mut c_void,
    meter: *mut c_void,
    tempo: *mut c_void,
    telemetry: *mut HirariEngineTelemetry,
    telemetry_active: AtomicU32,
    telemetry_lock: AtomicBool,
    telemetry_ids: *mut u32,
    telemetry_left: *mut *const f32,
    telemetry_right: *mut *const f32,
    click: *mut [[f32; MAX_BLOCK_FRAMES]; 2],
}

// The handles belong to one engine, and only its serialized audio callback and
// control thread touch this state.
unsafe impl Send for EngineBlockState {}
unsafe impl Sync for EngineBlockState {}

fn block(state: *const c_void) -> Option<&'static EngineBlockState> {
    unsafe { state.cast::<EngineBlockState>().as_ref() }
}

impl EngineBlockState {
    fn runtime(&self, track: *mut c_void) -> Option<&'static TrackRuntime> {
        let op = self.ops.runtime?;
        track_runtime::runtime(unsafe { op(track) })
    }

    fn is_bus(&self, track: *mut c_void) -> bool {
        let Some(op) = self.ops.is_bus else {
            return false;
        };
        (unsafe { op(track) }) != 0
    }

    fn record(&self, track: *mut c_void, playhead: u64) {
        if let Some(op) = self.ops.record {
            unsafe { op(track, playhead) };
        }
    }

    fn click(&self, side: usize) -> *mut f32 {
        let scratch = unsafe { &mut *self.click };
        scratch[side].as_mut_ptr()
    }

    /// The stereo master window the device callback owns, or null when the
    /// declared block does not fit the buffer holding it.
    fn window(
        &self,
        output: *mut c_void,
        off: u32,
        frames: u32,
    ) -> Option<(*mut f32, *mut f32, *const *mut f32, u32)> {
        let (planes, count, capacity) = track_runtime::channels_of(output);
        if planes.is_null() || count < 2 || capacity < off.checked_add(frames)? {
            return None;
        }
        let planes = unsafe { slice::from_raw_parts(planes, count as usize) };
        if planes[0].is_null() || planes[1].is_null() {
            return None;
        }
        let offset = off as usize;
        Some((
            unsafe { planes[0].add(offset) },
            unsafe { planes[1].add(offset) },
            planes.as_ptr(),
            count,
        ))
    }

    /// Feeds each track's post-graph block into its one-block delayed routing
    /// edges, in published graph order.
    unsafe fn capture_feedback(&self, frames: u32, view: View) {
        for &track in view.active() {
            if track.is_null() {
                continue;
            }
            let Some(runtime) = self.runtime(track) else {
                continue;
            };
            let Some(tap) = runtime.work(frames) else {
                continue;
            };
            unsafe {
                hirari_routing_engine_capture_feedback(
                    self.routing,
                    runtime.track_id(),
                    tap.left,
                    tap.right,
                    frames,
                )
            };
        }
    }

    /// Publishes per-track linear peaks through the double buffer. The payload
    /// stays strictly aligned with the active track order, so a track that
    /// cannot hold the block keeps its slot at silence.
    unsafe fn publish_peaks(&self, frames: u32, view: View) {
        let flag = self.telemetry_lock.swap(true, Ordering::Acquire);
        debug_assert!(!flag, "the metering handoff must not re-enter the callback");
        let next = ((self.telemetry_active.load(Ordering::Relaxed) as usize) ^ 1) as isize;
        let telemetry = unsafe { &mut *self.telemetry.offset(next) };
        telemetry.count = view.count.min(MAX_TRACKS as u32);
        let slots = telemetry.count as usize;
        let ids = unsafe { slice::from_raw_parts_mut(self.telemetry_ids, slots) };
        let lefts = unsafe { slice::from_raw_parts_mut(self.telemetry_left, slots) };
        let rights = unsafe { slice::from_raw_parts_mut(self.telemetry_right, slots) };
        ids.fill(0);
        lefts.fill(std::ptr::null());
        rights.fill(std::ptr::null());
        for slot in 0..slots {
            let Some(&track) = view.active().get(slot) else {
                continue;
            };
            if track.is_null() {
                continue;
            }
            let Some(runtime) = self.runtime(track) else {
                continue;
            };
            let Some(tap) = runtime.work(frames) else {
                continue;
            };
            ids[slot] = runtime.track_id();
            lefts[slot] = tap.left;
            rights[slot] = tap.right;
        }
        unsafe {
            hirari_mixer_track_peaks(
                self.mixer_telemetry,
                ids.as_ptr(),
                lefts.as_ptr(),
                rights.as_ptr(),
                telemetry.count,
                frames,
                telemetry.peaks_left.as_mut_ptr(),
                telemetry.peaks_right.as_mut_ptr(),
            )
        };
        telemetry.version += 1;
        self.telemetry_active.store(next as u32, Ordering::Release);
        self.telemetry_lock.store(false, Ordering::Release);
    }

    /// The metronome belongs to the rendered monitor signal, not to a track
    /// node, so it is injected here and left for the master limiter to trim.
    /// Returns whether the click is part of this block.
    unsafe fn render_metronome(
        &self,
        frames: u32,
        playhead: u64,
        sample_rate: f64,
        playing: bool,
    ) -> bool {
        let block = frames as usize;
        let scratch = unsafe { &mut *self.click };
        scratch[0][..block].fill(0.0);
        scratch[1][..block].fill(0.0);
        if !playing || !unsafe { hirari_metronome_is_enabled(self.metronome) } {
            return false;
        }
        unsafe {
            hirari_metronome_process(
                self.metronome,
                self.click(0),
                self.click(1),
                frames,
                playhead,
                sample_rate,
                hirari_tempo_map_get_current_bpm(self.tempo) as f64,
            )
        };
        true
    }

    fn find_track(&self, track_id: u32, view: View) -> Option<*mut c_void> {
        view.active()
            .iter()
            .copied()
            .filter(|&track| !track.is_null())
            .find(|&track| {
                self.runtime(track)
                    .is_some_and(|runtime| runtime.track_id() == track_id)
            })
    }

    /// Control Room is strictly device monitoring: speaker trim and DIM are
    /// never baked into an offline master or stem render.
    unsafe fn render_control_room(
        &self,
        planes: *const *mut f32,
        channel_count: u32,
        off: u32,
        frames: u32,
        talkback_offset: u32,
        view: View,
    ) {
        if hirari_bounce_offline_active(self.bounce) != 0 {
            return;
        }
        let planes = unsafe { slice::from_raw_parts(planes, channel_count as usize) };
        let offset = off as usize;
        let active_cue_bus = unsafe { hirari_control_room_active_cue_bus(self.control_room) };
        let mut cue = unsafe { hirari_control_room_active_cue_output(self.control_room) };
        let dedicated_pair = cue >= 2;
        let cue_available = cue + 1 < channel_count;
        if !cue_available {
            cue = 0;
        }
        let index = cue as usize;
        if active_cue_bus != 0 && (!dedicated_pair || cue_available) {
            if let Some(bus) = self.find_track(active_cue_bus, view) {
                if let Some(runtime) = self.runtime(bus).filter(|_| self.is_bus(bus)) {
                    if let Some(tap) = runtime.work(frames) {
                        unsafe {
                            hirari_control_room_mix_cue(
                                tap.left,
                                tap.right,
                                self.click(0) as *const f32,
                                self.click(1) as *const f32,
                                planes[index].add(offset),
                                planes[index + 1].add(offset),
                                frames,
                                hirari_control_room_active_cue_gain(self.control_room),
                                hirari_control_room_active_cue_click(self.control_room),
                            )
                        };
                    }
                }
            }
        }
        let talkback =
            unsafe { hirari_input_blocks_take_talkback(self.input_blocks, talkback_offset, frames) };
        if dedicated_pair {
            // Keep the Main control-room monitor live while a selected Cue feeds
            // its own output pair; talkback stays on that pair so it cannot leak
            // into the control-room mix.
            unsafe {
                hirari_control_room_process_monitor_state(
                    self.control_room,
                    planes[0].add(offset),
                    planes[1].add(offset),
                    std::ptr::null(),
                    frames,
                );
            }
        }
        if !dedicated_pair || cue_available {
            unsafe {
                hirari_control_room_process_monitor_state(
                    self.control_room,
                    planes[index].add(offset),
                    planes[index + 1].add(offset),
                    talkback,
                    frames,
                )
            };
        }
    }
}

/// Runs the post-graph tail of one engine block.
///
/// # Safety
/// `state` comes from [`hirari_engine_block_create`], `output` is that engine's
/// block storage, and `active_tracks`/`track_count` is the graph window the
/// callback resolved at the head of the block.
#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_finish(
    state: *mut c_void,
    output: *mut c_void,
    off: u32,
    frames: u32,
    playhead: u64,
    sample_rate: f64,
    master_gain: f32,
    playing: u8,
    auto_record: u8,
    talkback_offset: u32,
    active_tracks: *const *mut c_void,
    track_count: u32,
    advance_out: *mut u32,
    overflow_out: *mut u8,
) {
    let advance = unsafe { advance_out.as_mut() };
    let overflow = unsafe { overflow_out.as_mut() };
    if let Some(value) = advance {
        *value = 0;
    }
    if let Some(value) = overflow {
        *value = 0;
    }
    // The click scratch holds one kMaxAudioBlockSize block; the engine's head
    // gate clamps the block length before it reaches the tail.
    let Some(state) = block(state).filter(|_| frames as usize <= MAX_BLOCK_FRAMES) else {
        return;
    };
    let Some((left, right, planes, channel_count)) = state.window(output, off, frames) else {
        return;
    };
    let view = View {
        tracks: active_tracks,
        count: track_count,
    };
    let playing = playing != 0;
    unsafe {
        state.capture_feedback(frames, view);
        state.publish_peaks(frames, view);
        let click = state.render_metronome(frames, playhead, sample_rate, playing);
        hirari_master_output_process(
            left,
            right,
            state.click(0),
            state.click(1),
            frames,
            master_gain,
            click,
        );
        if !state.master_limiter.is_null() {
            hirari_master_limiter_process(state.master_limiter, left, right, frames as usize, true);
        }
        hirari_master_meter_process(
            state.meter,
            left as *const f32,
            right as *const f32,
            frames as usize,
        );
        state.render_control_room(planes, channel_count, off, frames, talkback_offset, view);
        if playing && auto_record > 0 {
            for &track in view.active() {
                if !track.is_null() {
                    state.record(track, playhead);
                }
            }
        }
    }
    if !playing {
        return;
    }
    if playhead > u64::MAX - frames as u64 {
        if let Some(value) = unsafe { overflow_out.as_mut() } {
            *value = 1;
        }
    } else if let Some(value) = unsafe { advance_out.as_mut() } {
        *value = frames;
    }
}

/// Binds an engine's block tail. Every handle must outlive the returned state.
///
/// # Safety
/// `ops` is read once; the handles belong to this one engine.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_create(
    routing: *mut c_void,
    bounce: *mut c_void,
    input_blocks: *mut c_void,
    control_room: *mut c_void,
    mixer_telemetry: *mut c_void,
    metronome: *mut c_void,
    master_limiter: *mut c_void,
    meter: *mut c_void,
    tempo: *mut c_void,
    ops: *const HirariEngineBlockOps,
) -> *mut c_void {
    // The tail writes these from the audio callback only, so they are bound once
    // here instead of rebuilt for every block.
    let mut telemetry: Box<[HirariEngineTelemetry]> =
        vec![HirariEngineTelemetry::default(); 2].into();
    let telemetry_ptr = telemetry.as_mut_ptr();
    let mut ids: Box<[u32]> = vec![0u32; MAX_TRACKS].into();
    let ids_ptr = ids.as_mut_ptr();
    let mut lefts: Box<[*const f32]> = vec![std::ptr::null(); MAX_TRACKS].into();
    let lefts_ptr = lefts.as_mut_ptr();
    let mut rights: Box<[*const f32]> = vec![std::ptr::null(); MAX_TRACKS].into();
    let rights_ptr = rights.as_mut_ptr();
    let click: Box<[[f32; MAX_BLOCK_FRAMES]; 2]> =
        Box::new([[0.0; MAX_BLOCK_FRAMES], [0.0; MAX_BLOCK_FRAMES]]);
    let click_ptr = Box::into_raw(click);
    std::mem::forget(telemetry);
    std::mem::forget(ids);
    std::mem::forget(lefts);
    std::mem::forget(rights);
    Box::into_raw(Box::new(EngineBlockState {
        ops: if ops.is_null() {
            HirariEngineBlockOps::default()
        } else {
            unsafe { *ops }
        },
        routing,
        bounce,
        input_blocks,
        control_room,
        mixer_telemetry,
        metronome,
        master_limiter,
        meter,
        tempo,
        telemetry: telemetry_ptr,
        telemetry_active: AtomicU32::new(0),
        telemetry_lock: AtomicBool::new(false),
        telemetry_ids: ids_ptr,
        telemetry_left: lefts_ptr,
        telemetry_right: rights_ptr,
        click: click_ptr,
    }))
    .cast()
}

/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_destroy(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    let state = unsafe { Box::from_raw(state.cast::<EngineBlockState>()) };
    unsafe {
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.telemetry,
            2,
        )));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.telemetry_ids,
            MAX_TRACKS,
        )));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.telemetry_left,
            MAX_TRACKS,
        )));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.telemetry_right,
            MAX_TRACKS,
        )));
        drop(Box::from_raw(state.click));
    }
}

/// The buffer index a control reader should copy.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`].
#[no_mangle]
pub extern "C" fn hirari_engine_telemetry_active_index(state: *const c_void) -> u32 {
    block(state).map_or(0, |state| state.telemetry_active.load(Ordering::Acquire))
}

/// Copies one published metering block, or reports that the audio callback
/// holds it.
///
/// # Safety
/// `state` must be null or a pointer from [`hirari_engine_block_create`] and
/// `out` writable when non-null.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_telemetry_snapshot(
    state: *const c_void,
    index: u32,
    out: *mut HirariEngineTelemetry,
) -> bool {
    let Some(state) = block(state) else {
        return false;
    };
    if index >= 2 || out.is_null() {
        return false;
    }
    if state.telemetry_lock.swap(true, Ordering::Acquire) {
        return false;
    }
    unsafe { *out = *state.telemetry.offset(index as isize) };
    state.telemetry_lock.store(false, Ordering::Release);
    true
}
