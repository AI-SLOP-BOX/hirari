//! Rust owns the engine's block: the graph walk and the signal that leaves it.
//!
//! The C++ callback used to stage every track, inject feedback, feed the live
//! driver input, publish the routing destination table, walk the graph in
//! process order and fan each track out to its sends and the master, then
//! capture feedback, publish metering, render the metronome, scale and limit
//! the master, measure it, mix the Control Room cue and advance the playhead --
//! 300 lines of realtime policy inlined in `processBlock`, plus its click
//! scratch and the double-buffered telemetry handoff. The engine binds its
//! handles once into this state and calls [`hirari_engine_block_route`] and
//! [`hirari_engine_block_finish`] around the device buffer it is rendering.
//!
//! Tracks stay C++ objects, so the four things this state cannot reach through
//! a handle -- a track's runtime, its solo flag, its bus stage and its armed
//! recorder -- travel as a published operations table. Every tap buffer the walk
//! reads is already bound in the track's [`TrackRuntime`].

use crate::audio_buffer_ops::hirari_audio_add_sanitized_stereo;
use crate::bounce_driver::{
    hirari_bounce_include_inserts, hirari_bounce_offline_active, hirari_bounce_pre_fader,
    hirari_bounce_target_active, hirari_bounce_target_id,
};
use crate::bus_audio::{
    hirari_bus_audio_accumulate, hirari_bus_audio_read, hirari_bus_audio_replace_post,
    hirari_bus_system_clear,
};
use crate::control_room_audio::hirari_control_room_mix_cue;
use crate::control_room_runtime::{
    hirari_control_room_active_cue_bus, hirari_control_room_active_cue_click,
    hirari_control_room_active_cue_gain, hirari_control_room_active_cue_output,
    hirari_control_room_is_cue_bus, hirari_control_room_process_monitor_state,
};
use crate::engine_input_block::{hirari_input_blocks_take_monitor, hirari_input_blocks_take_talkback};
use crate::master_limiter::hirari_master_limiter_process;
use crate::master_meter::hirari_master_meter_process;
use crate::master_output::hirari_master_output_process;
use crate::metronome_renderer::{hirari_metronome_is_enabled, hirari_metronome_process};
use crate::mixer_telemetry::hirari_mixer_track_peaks;
use crate::routing_engine_core::{
    hirari_routing_engine_capture_feedback, hirari_routing_engine_enter_send_pdc_read,
    hirari_routing_engine_inject_feedback, hirari_routing_engine_leave_send_pdc_read,
    hirari_routing_engine_process_track_fanout, hirari_routing_engine_resolve_process_order,
    hirari_routing_engine_should_process,
};
use crate::routing_fanout::{BusPreFxAdd, RoutingTrackDestination};
use crate::sidechain_manager::hirari_sidechain_engine_publish_source;
use crate::tempo::hirari_tempo_map_get_current_bpm;
use crate::track_runtime::{self, TrackRuntime};
use std::ffi::c_void;
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// `HirariUnifiedEngine::kMaxTracks`, and the width of one metering payload.
const MAX_TRACKS: usize = 256;
/// `HirariUnifiedEngine::TelemetryData::spectrum` bins.
const SPECTRUM_BINS: usize = 512;
/// `HirariUnifiedEngine::kMaxAudioBlockSize`; the click scratch holds one block.
const MAX_BLOCK_FRAMES: usize = 16_384;
/// `BusTrack::kMaxSamples`, the cap on one bus stage and on Send PDC scratch.
const MAX_BUS_FRAMES: usize = 8_192;
/// `RoutingEngine::kMaxNodes`, on top of the tracks a graph order can insert.
const MAX_GRAPH_NODES: usize = MAX_TRACKS + 128;

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

/// The per-track operations the engine block needs from C++.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariEngineBlockOps {
    /// The track's bound [`TrackRuntime`] handle.
    pub runtime: Option<unsafe extern "C" fn(*mut c_void) -> *mut c_void>,
    /// The track's AUX/BUS stage, or null when the track is not a bus.
    pub bus_state: Option<unsafe extern "C" fn(*mut c_void) -> *mut c_void>,
    /// Non-zero while the track carries solo.
    pub solo: Option<unsafe extern "C" fn(*mut c_void) -> u8>,
    /// Captures the fader and pan of one armed recorder.
    pub record: Option<unsafe extern "C" fn(*mut c_void, u64)>,
}

/// The graph eligibility the whole block shares: an offline target isolates one
/// stem, otherwise a solo anywhere mutes every track that does not carry it.
#[derive(Clone, Copy)]
struct Gating {
    offline_target_active: bool,
    offline_target_id: u32,
    any_solo: bool,
}

/// Routes one Send edge into a bus's pre-FX stage. The destination table
/// carries the bus handle Rust already owns, so the fanout kernel needs no C++
/// callback of its own.
unsafe extern "C" fn add_bus_pre_fx(
    context: *mut c_void,
    left: *const f32,
    right: *const f32,
    frames: u32,
    gain: f32,
) -> bool {
    if context.is_null() || frames == 0 || frames > MAX_BUS_FRAMES as u32 {
        return false;
    }
    unsafe { hirari_bus_audio_accumulate(context, left, right, frames, gain) }
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
    bus_system: *mut c_void,
    sidechain: *mut c_void,
    telemetry: *mut HirariEngineTelemetry,
    telemetry_active: AtomicU32,
    telemetry_lock: AtomicBool,
    telemetry_ids: *mut u32,
    telemetry_left: *mut *const f32,
    telemetry_right: *mut *const f32,
    click: *mut [[f32; MAX_BLOCK_FRAMES]; 2],
    send_pdc: *mut [[f32; MAX_BUS_FRAMES]; 2],
    destinations: *mut [RoutingTrackDestination; MAX_TRACKS],
    graph_ids: *mut [u32; MAX_TRACKS],
    process_order: *mut [u32; MAX_GRAPH_NODES],
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

    fn bus_state(&self, track: *mut c_void) -> *mut c_void {
        let Some(op) = self.ops.bus_state else {
            return ptr::null_mut();
        };
        unsafe { op(track) }
    }

    fn solo(&self, track: *mut c_void) -> bool {
        let Some(op) = self.ops.solo else {
            return false;
        };
        unsafe { op(track) != 0 }
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
                let bus_state = self.bus_state(bus);
                if let Some(runtime) = self.runtime(bus).filter(|_| !bus_state.is_null()) {
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

    /// Graph eligibility for one track, read through the routing kernel the
    /// engine's solo and offline-target state feeds.
    fn should_process(&self, track: *mut c_void, gating: &Gating) -> bool {
        let Some(runtime) = self.runtime(track) else {
            return false;
        };
        unsafe {
            hirari_routing_engine_should_process(
                self.routing,
                runtime.track_id(),
                gating.offline_target_active,
                gating.offline_target_id,
                gating.any_solo,
                self.solo(track),
            )
        }
    }

    /// Publishes one block's tempo and playback state, then empties every work
    /// buffer so upstream tracks accumulate into a known-zero tap.
    unsafe fn stage_tracks(&self, frames: u32, view: View, playing: bool) {
        let bpm = unsafe { hirari_tempo_map_get_current_bpm(self.tempo) } as f64;
        for &track in view.active() {
            if track.is_null() {
                continue;
            }
            if let Some(runtime) = self
                .runtime(track)
                .filter(|runtime| runtime.work(frames).is_some())
            {
                runtime.stage(frames, bpm, playing);
            }
        }
    }

    /// Feedback edges are explicitly delayed by one engine block. They are
    /// injected before any graph node runs, so a cycle cannot recurse on the
    /// current block or allocate on the audio thread.
    unsafe fn inject_feedback(&self, frames: u32, view: View) {
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
                hirari_routing_engine_inject_feedback(
                    self.routing,
                    runtime.track_id(),
                    tap.left,
                    tap.right,
                    frames,
                )
            };
        }
    }

    /// Feeds the live driver input into the selected monitor track before graph
    /// processing. The staged planes were copied by the driver callback, so this
    /// path performs no allocation or locking.
    unsafe fn feed_monitor_input(&self, frames: u32, monitor_offset: u32, view: View) {
        let monitor =
            unsafe { hirari_input_blocks_take_monitor(self.input_blocks, monitor_offset, frames) };
        if monitor.left.is_null() {
            return;
        }
        let Some(track) = self.find_track(monitor.track_id, view) else {
            return;
        };
        if let Some(runtime) = self.runtime(track) {
            runtime.add_input(monitor.left.cast_const(), monitor.right.cast_const(), frames);
        }
    }

    /// Publishes the destination table one fanout call reads, in graph order.
    /// The scratch it lives in is bound once, so no block allocates.
    unsafe fn publish_destinations(&self, frames: u32, view: View, gating: &Gating) -> usize {
        let table = unsafe { &mut *self.destinations };
        let mut count = 0usize;
        for &track in view.active() {
            if track.is_null() {
                continue;
            }
            let Some(runtime) = self.runtime(track) else {
                continue;
            };
            let (channels, channel_count) = runtime.work_channels(frames);
            if channels.is_null() {
                continue;
            }
            let bus_state = self.bus_state(track);
            table[count] = RoutingTrackDestination {
                id: runtime.track_id(),
                can_process: u8::from(self.should_process(track, gating)),
                is_bus: u8::from(!bus_state.is_null()),
                channel_count,
                work_channels: channels,
                bus_context: bus_state,
            };
            count += 1;
        }
        count
    }

    /// Applies one processed track's normal routes and parallel bus sends.
    #[allow(clippy::too_many_arguments)]
    unsafe fn route_to_next_stages(
        &self,
        runtime: &TrackRuntime,
        gating: &Gating,
        master_left: *mut f32,
        master_right: *mut f32,
        destinations: &[RoutingTrackDestination],
        frames: u32,
    ) {
        // Cue buses are isolated monitor sources. Their Send inputs and insert
        // chain run normally, but their signal is selected only at the Control
        // Room boundary, never summed into the Main mix.
        if !gating.offline_target_active
            && unsafe { hirari_control_room_is_cue_bus(self.control_room, runtime.track_id()) }
        {
            return;
        }
        if gating.offline_target_active
            && gating.offline_target_id != 0
            && runtime.track_id() == gating.offline_target_id
        {
            let tap = if hirari_bounce_pre_fader(self.bounce) != 0 {
                runtime.pre_fader(frames)
            } else if hirari_bounce_include_inserts(self.bounce) == 0 {
                runtime.pre_insert(frames)
            } else {
                runtime.work(frames)
            };
            if let Some(tap) = tap {
                unsafe {
                    hirari_audio_add_sanitized_stereo(
                        master_left,
                        master_right,
                        tap.left.cast_const(),
                        tap.right.cast_const(),
                        frames,
                    )
                };
            }
            return;
        }
        let send_pre = runtime.send_pre(frames);
        let send_post = runtime.send_post(frames);
        let (send_pre_left, send_pre_right) = send_pre.map_or((ptr::null(), ptr::null()), |tap| {
            (tap.left.cast_const(), tap.right.cast_const())
        });
        let (send_post_left, send_post_right) =
            send_post.map_or((ptr::null(), ptr::null()), |tap| {
                (tap.left.cast_const(), tap.right.cast_const())
            });
        let (source_channels, source_channel_count) = runtime.work_channels(frames);
        // One state-owned scratch pair is reused for each sequential Send edge;
        // delay histories live in the routing kernel and nothing here allocates.
        let scratch = unsafe { &mut *self.send_pdc };
        unsafe {
            hirari_routing_engine_process_track_fanout(
                self.routing,
                runtime.track_id(),
                source_channels.cast(),
                source_channel_count,
                send_pre_left,
                send_pre_right,
                send_post_left,
                send_post_right,
                runtime.delay_samples(),
                destinations.as_ptr(),
                destinations.len(),
                master_left,
                master_right,
                scratch[0].as_mut_ptr(),
                scratch[1].as_mut_ptr(),
                frames,
                MAX_BUS_FRAMES as u32,
                Some(add_bus_pre_fx as BusPreFxAdd),
            );
        }
    }

    /// Walks the graph in resolved process order: upstream tracks have already
    /// accumulated into this node's work buffer, so the node renders what it was
    /// given, publishes itself as a sidechain source and fans out.
    unsafe fn walk_graph(
        &self,
        frames: u32,
        playhead: u64,
        sample_rate: u32,
        generation: u64,
        view: View,
        gating: &Gating,
        master_left: *mut f32,
        master_right: *mut f32,
        destinations: &[RoutingTrackDestination],
    ) {
        let ids = unsafe { &mut *self.graph_ids };
        let mut id_count = 0usize;
        for &track in view.active() {
            if track.is_null() {
                continue;
            }
            if let Some(runtime) = self.runtime(track) {
                ids[id_count] = runtime.track_id();
                id_count += 1;
            }
        }
        let order = unsafe { &mut *self.process_order };
        let mut order_count = 0u32;
        if !unsafe {
            hirari_routing_engine_resolve_process_order(
                self.routing,
                ids.as_ptr(),
                id_count,
                order.as_mut_ptr(),
                order.len(),
                &mut order_count,
            )
        } {
            // A graph the topology cannot order still renders in published
            // order rather than dropping every track's block.
            order_count = id_count as u32;
            order[..id_count].copy_from_slice(&ids[..id_count]);
        }
        for index in 0..order_count as usize {
            let Some(track) = self.find_track(order[index], view) else {
                continue;
            };
            if !self.should_process(track, gating) {
                continue;
            }
            let Some(runtime) = self.runtime(track) else {
                continue;
            };
            let Some(work) = runtime.work(frames) else {
                continue;
            };
            let bus_state = self.bus_state(track);
            if !bus_state.is_null() && frames != 0 && frames <= MAX_BUS_FRAMES as u32 {
                unsafe { hirari_bus_audio_read(bus_state, work.left, work.right, frames, false) };
            }
            // The graph publishes the node it just rendered whether or not the
            // chain ran: a muted or frozen track still owns its bus stage and
            // still feeds downstream sidechain and Send edges.
            let _ = runtime.process_accumulated(frames, playhead);
            if !bus_state.is_null() && frames != 0 && frames <= MAX_BUS_FRAMES as u32 {
                unsafe {
                    hirari_bus_audio_replace_post(
                        bus_state,
                        work.left.cast_const(),
                        work.right.cast_const(),
                        frames,
                    )
                };
            }
            let (pre_left, pre_right) = runtime
                .pre_insert(frames)
                .map_or((ptr::null(), ptr::null()), |tap| {
                    (tap.left.cast_const(), tap.right.cast_const())
                });
            let (post_left, post_right) = runtime
                .pre_fader(frames)
                .map_or((ptr::null(), ptr::null()), |tap| {
                    (tap.left.cast_const(), tap.right.cast_const())
                });
            unsafe {
                hirari_sidechain_engine_publish_source(
                    self.sidechain,
                    runtime.track_id(),
                    pre_left,
                    pre_right,
                    post_left,
                    post_right,
                    work.left.cast_const(),
                    work.right.cast_const(),
                    frames,
                    sample_rate,
                    generation,
                );
            }
            self.route_to_next_stages(
                runtime,
                gating,
                master_left,
                master_right,
                destinations,
                frames,
            );
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

/// Runs the graph half of one engine block: staging, feedback injection, the
/// live driver input, the routing destination table and the process-order walk
/// that fills the master window and every bus stage.
///
/// # Safety
/// `state` comes from [`hirari_engine_block_create`], `output` is that engine's
/// block storage, and `active_tracks`/`track_count` is the graph window the
/// callback resolved at the head of the block.
#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_block_route(
    state: *mut c_void,
    output: *mut c_void,
    off: u32,
    frames: u32,
    playhead: u64,
    sample_rate: f64,
    monitor_offset: u32,
    generation: u64,
    playing: u8,
    active_tracks: *const *mut c_void,
    track_count: u32,
) {
    let Some(state) = block(state).filter(|_| frames as usize <= MAX_BLOCK_FRAMES) else {
        return;
    };
    let Some((master_left, master_right, _, _)) = state.window(output, off, frames) else {
        return;
    };
    let view = View {
        tracks: active_tracks,
        count: track_count,
    };
    let gating = Gating {
        offline_target_active: hirari_bounce_target_active(state.bounce) != 0,
        offline_target_id: hirari_bounce_target_id(state.bounce),
        any_solo: view
            .active()
            .iter()
            .copied()
            .filter(|track| !track.is_null())
            .any(|track| state.solo(track)),
    };
    unsafe {
        hirari_bus_system_clear(state.bus_system.cast_const(), frames);
        state.stage_tracks(frames, view, playing != 0);
        state.inject_feedback(frames, view);
        state.feed_monitor_input(frames, monitor_offset, view);
        let destination_count = state.publish_destinations(frames, view, &gating);
        let destinations = slice::from_raw_parts(state.destinations.cast(), destination_count);
        // The Send PDC delay histories are Rust-owned; this section keeps a
        // control-thread mutation from retiring them mid-walk.
        hirari_routing_engine_enter_send_pdc_read(state.routing);
        state.walk_graph(
            frames,
            playhead,
            sample_rate.max(1.0) as u32,
            generation,
            view,
            &gating,
            master_left,
            master_right,
            destinations,
        );
        hirari_routing_engine_leave_send_pdc_read(state.routing);
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
    bus_system: *mut c_void,
    sidechain: *mut c_void,
    ops: *const HirariEngineBlockOps,
) -> *mut c_void {
    // The walk writes these from the audio callback only, so they are bound once
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
    let send_pdc: Box<[[f32; MAX_BUS_FRAMES]; 2]> =
        Box::new([[0.0; MAX_BUS_FRAMES], [0.0; MAX_BUS_FRAMES]]);
    let send_pdc_ptr = Box::into_raw(send_pdc);
    let seed = RoutingTrackDestination {
        id: 0,
        can_process: 0,
        is_bus: 0,
        channel_count: 0,
        work_channels: ptr::null(),
        bus_context: ptr::null_mut(),
    };
    let mut destinations: Box<[RoutingTrackDestination]> =
        vec![seed; MAX_TRACKS].into();
    let destinations_ptr = destinations.as_mut_ptr();
    let mut graph_ids: Box<[u32]> = vec![0u32; MAX_TRACKS].into();
    let graph_ids_ptr = graph_ids.as_mut_ptr();
    let mut process_order: Box<[u32]> = vec![0u32; MAX_GRAPH_NODES].into();
    let process_order_ptr = process_order.as_mut_ptr();
    std::mem::forget(telemetry);
    std::mem::forget(ids);
    std::mem::forget(lefts);
    std::mem::forget(rights);
    std::mem::forget(destinations);
    std::mem::forget(graph_ids);
    std::mem::forget(process_order);
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
        bus_system,
        sidechain,
        telemetry: telemetry_ptr,
        telemetry_active: AtomicU32::new(0),
        telemetry_lock: AtomicBool::new(false),
        telemetry_ids: ids_ptr,
        telemetry_left: lefts_ptr,
        telemetry_right: rights_ptr,
        click: click_ptr,
        send_pdc: send_pdc_ptr,
        destinations: destinations_ptr.cast(),
        graph_ids: graph_ids_ptr.cast(),
        process_order: process_order_ptr.cast(),
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
        drop(Box::from_raw(state.send_pdc));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.destinations,
            MAX_TRACKS,
        )));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.graph_ids,
            MAX_TRACKS,
        )));
        drop(Box::from_raw(slice::from_raw_parts_mut(
            state.process_order,
            MAX_GRAPH_NODES,
        )));
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
