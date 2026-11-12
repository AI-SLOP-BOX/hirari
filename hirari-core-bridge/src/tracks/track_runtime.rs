//! Rust-owned per-track realtime state and the block pipeline entry.
//!
//! Track used to rebuild a 35-field process request and a 33-argument finalise
//! call on the audio thread, then copy the results back into its own atomics.
//! The runtime binds the track's DSP handles and its automation scratch once,
//! keeps the values the audio thread reads as atomics, and renders a whole
//! block behind a single call.

use crate::audio_buffer_ops::{
    hirari_audio_buffer_render_frozen, hirari_audio_buffer_storage_add_channels_from,
    hirari_audio_buffer_storage_array_write, hirari_audio_buffer_storage_channels,
    hirari_audio_buffer_storage_reserve_buffer, hirari_audio_buffer_storage_resize,
    hirari_audio_buffer_storage_samples,
};
use crate::built_in_midi_instrument::hirari_builtin_midi_instrument_process;
use crate::channel_strip::{
    hirari_channel_strip_reset, hirari_channel_strip_sample_rate,
    hirari_channel_strip_set_sample_rate,
};
use crate::effect_chain_host::{
    hirari_effect_chain_midi_instrument_count, hirari_effect_chain_parameter_automation,
    hirari_effect_chain_process, hirari_effect_chain_reset, hirari_effect_chain_set_sample_rate,
    TimedParameterEvent,
};
use crate::midi_buffer_ops::{
    hirari_midi_buffer_add, hirari_midi_buffer_clear, hirari_midi_buffer_event_count,
    hirari_midi_buffer_event_data, hirari_midi_buffer_remaining_capacity,
    hirari_midi_buffer_sort_owned,
};
use crate::realtime_automation::{
    hirari_plugin_automation_render_block, hirari_realtime_automation_evaluate,
    NativeTimedParameterEvent,
};
use crate::region_store::hirari_region_store_prepare;
use crate::track_automation_store::{
    hirari_track_automation_store_hint, hirari_track_automation_store_plugin_lane_views,
    hirari_track_automation_store_points, hirari_track_automation_store_reset_hints,
};
use crate::track_frozen_store::{
    hirari_track_frozen_store_clear, hirari_track_frozen_store_current,
    hirari_track_frozen_store_is_frozen, hirari_track_frozen_store_retire,
    hirari_track_frozen_store_sample_rate, hirari_track_frozen_store_total_samples,
};
use crate::track_holographic_panner::hirari_track_holographic_panner_set_sample_rate;
use crate::track_midi_articulation::{
    hirari_track_midi_flush_scheduled_note_offs, GeneratedMidiEvent,
};
use crate::track_output::{finalize_track_output, TrackFinalizeInput};
use crate::track_pdc_delay::hirari_track_pdc_delay_reset;
use crate::track_process::MAX_TRACK_BLOCK_FRAMES;
use crate::track_region_render::hirari_track_render_regions;
use std::ffi::c_void;
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

/// Per-block plugin automation capacity, allocated once per track instead of
/// on the audio thread's stack.
const PLUGIN_EVENTS_PER_BLOCK: usize = 4096;
/// The bound delay line both the manual offset and plugin PDC share.
const MAX_DELAY_SAMPLES: u32 = 8192;

/// The note-off events one block may schedule, matching the bound the C++
/// track used to size its stack scratch with.
const MIDI_EVENTS_PER_BLOCK: usize = 1024;

/// The `Track::Type` values that get a built-in voice when no instrument
/// plugin is loaded.
const TRACK_TYPE_MIDI: u32 = 1;
const TRACK_TYPE_INSTRUMENT: u32 = 2;

pub struct TrackRuntime {
    track_id: u32,
    track_type: u32,
    chain: *mut c_void,
    channel_strip: *mut c_void,
    automation_store: *mut c_void,
    frozen_store: *mut c_void,
    panner: *mut c_void,
    pdc_delay: *mut c_void,
    work: *mut c_void,
    pre_insert: *mut c_void,
    pre_fader: *mut c_void,
    send_pre: *mut c_void,
    send_post: *mut c_void,
    midi: *mut c_void,
    articulation: *mut c_void,
    built_in_instrument: *mut c_void,
    midi_events: *mut GeneratedMidiEvent,
    plugin_events: *mut NativeTimedParameterEvent,
    muted: AtomicBool,
    phase_invert: AtomicBool,
    transport_playing: AtomicBool,
    transport_bpm: AtomicU64,
    pdc_requested: AtomicU32,
    track_delay_samples: AtomicU32,
    spatial_x: AtomicU32,
    spatial_y: AtomicU32,
    spatial_z: AtomicU32,
    low_gain: AtomicU32,
    high_gain: AtomicU32,
    low_state_left: AtomicU32,
    low_state_right: AtomicU32,
    region_inputs: AtomicPtr<c_void>,
    region_count: AtomicUsize,
    region_published: AtomicU8,
    /// Sidechain engine handle for sandbox delivery; the block host refreshes
    /// it before each track pass so workers read this block's sources.
    sidechain_gate: AtomicPtr<c_void>,
}

// The handles are only dereferenced from this track's audio callback and from
// the control thread that owns the track, both of which the host serialises.
unsafe impl Send for TrackRuntime {}
unsafe impl Sync for TrackRuntime {}

/// One track tap's writable stereo planes at a block length. A buffer the engine
/// cannot address as stereo stays unpublished.
pub(crate) struct Tap {
    pub(crate) left: *mut f32,
    pub(crate) right: *mut f32,
}

fn stored(value: f32) -> AtomicU32 {
    AtomicU32::new(value.to_bits())
}

fn read(state: &AtomicU32) -> f32 {
    f32::from_bits(state.load(Ordering::Relaxed))
}

pub(crate) fn channels_of(handle: *mut c_void) -> (*const *mut f32, u32, u32) {
    (
        unsafe { hirari_audio_buffer_storage_array_write(handle as *const _) },
        unsafe { hirari_audio_buffer_storage_channels(handle as *const _) },
        unsafe { hirari_audio_buffer_storage_samples(handle as *const _) },
    )
}

pub(crate) fn clear_planes(channels: *const *mut f32, count: u32, frames: usize) {
    if channels.is_null() || frames == 0 {
        return;
    }
    for &plane in unsafe { slice::from_raw_parts(channels, count as usize) } {
        if !plane.is_null() {
            unsafe { ptr::write_bytes(plane, 0, frames) };
        }
    }
}

impl TrackRuntime {
    /// The engine's block walk addresses a track through its runtime, so the
    /// identity and the tap buffers it publishes have to be readable natively.
    pub(crate) fn track_id(&self) -> u32 {
        self.track_id
    }

    /// Refreshes the sidechain engine handle used for sandbox delivery in
    /// the next track pass. Null disables sidechain input.
    pub(crate) fn set_sidechain_gate(&self, gate: *mut c_void) {
        self.sidechain_gate.store(gate, Ordering::Release);
    }

    /// The work buffer the graph-visible stages read at one block length. A
    /// buffer that cannot be addressed as stereo at this length publishes
    /// nothing, which is the same gate the C++ `canProcess` check applied.
    pub(crate) fn work(&self, frames: u32) -> Option<Tap> {
        self.window(self.work, frames)
    }

    /// The pre-fader tap: one Send edge's pre-fader source and the same buffer
    /// the post-FX sidechain reads.
    pub(crate) fn pre_fader(&self, frames: u32) -> Option<Tap> {
        self.window(self.pre_fader, frames)
    }

    /// The pre-insert tap: the offline render tap and the pre-FX sidechain.
    pub(crate) fn pre_insert(&self, frames: u32) -> Option<Tap> {
        self.window(self.pre_insert, frames)
    }

    pub(crate) fn send_pre(&self, frames: u32) -> Option<Tap> {
        self.window(self.send_pre, frames)
    }

    pub(crate) fn send_post(&self, frames: u32) -> Option<Tap> {
        self.window(self.send_post, frames)
    }

    /// The work-buffer channel table one routing destination publishes.
    pub(crate) fn work_channels(&self, frames: u32) -> (*const *mut f32, u32) {
        self.tap(self.work, frames)
    }

    /// The manual timing offset the fanout delays this track's master route by.
    pub(crate) fn delay_samples(&self) -> u32 {
        self.track_delay_samples.load(Ordering::Relaxed)
    }

    /// Publishes this block's tempo and playback flag, then empties the work
    /// buffer so upstream tracks can accumulate into it. A non-finite tempo
    /// keeps the previous block's value instead of inventing 120 BPM.
    pub(crate) fn stage(&self, frames: u32, bpm: f64, playing: bool) {
        let bpm = if bpm.is_finite() && bpm > 0.0 {
            bpm.clamp(1.0, 999.0)
        } else {
            f64::from_bits(self.transport_bpm.load(Ordering::Relaxed))
        };
        self.transport_bpm.store(bpm.to_bits(), Ordering::Relaxed);
        self.transport_playing.store(playing, Ordering::Relaxed);
        let (channels, channel_count, capacity) = channels_of(self.work);
        if frames == 0 || channel_count == 0 {
            return;
        }
        clear_planes(channels, channel_count, (frames as usize).min(capacity as usize));
    }

    /// Sums the live driver input into the track the monitor is routed to.
    pub(crate) fn add_input(&self, left: *const f32, right: *const f32, frames: u32) -> bool {
        let (channels, channel_count, capacity) = channels_of(self.work);
        if frames == 0
            || channels.is_null()
            || left.is_null()
            || right.is_null()
            || channel_count == 0
            || capacity < frames
        {
            return false;
        }
        unsafe {
            hirari_audio_buffer_storage_add_channels_from(self.work.cast_const(), left, right, frames)
        };
        true
    }

    /// Runs the insert chain and output stage over the block that upstream
    /// tracks already accumulated, without clearing it first.
    pub(crate) fn process_accumulated(&self, frames: u32, playhead: u64) -> bool {
        let (channels, channel_count, capacity) = channels_of(self.work);
        if channels.is_null() || frames == 0 || channel_count < 2 || capacity < frames {
            return false;
        }
        unsafe { self.process(self.work, frames, playhead, false) }
    }

    /// Retunes the track for a new device format and returns why a block growth
    /// could not happen; see [`hirari_track_runtime_prepare`].
    pub(crate) fn prepare(&self, region_key: *mut c_void, sample_rate: f64, block_frames: u32) -> u8 {
        if !sample_rate.is_finite() || sample_rate <= 0.0 || block_frames == 0 {
            return TrackPrepare::Ready.status();
        }
        unsafe {
            hirari_region_store_prepare(region_key, sample_rate, block_frames);
            hirari_channel_strip_set_sample_rate(self.channel_strip, sample_rate);
            hirari_track_holographic_panner_set_sample_rate(self.panner, sample_rate);
            hirari_channel_strip_reset(self.channel_strip);
        }
        // The order the track's own preparation used: the work block first, so a
        // declined growth still leaves the taps the callback addresses
        // consistent.
        for storage in [
            self.work,
            self.pre_fader,
            self.send_pre,
            self.send_post,
            self.pre_insert,
        ] {
            if storage.is_null() {
                continue;
            }
            let outcome = TrackPrepare::from_reserve(unsafe {
                hirari_audio_buffer_storage_reserve_buffer(storage, 2, block_frames)
            });
            if outcome == TrackPrepare::Ready {
                unsafe { hirari_audio_buffer_storage_resize(storage, 2, block_frames) };
                continue;
            }
            if outcome != TrackPrepare::Declined {
                return outcome.status();
            }
        }
        unsafe { hirari_effect_chain_set_sample_rate(self.chain, sample_rate, block_frames) };
        TrackPrepare::Ready.status()
    }

    /// Drops a freeze the new device rate cannot replay and retires its render.
    /// A 44.1 kHz snapshot must not survive a 48/96 kHz transition: it would
    /// silently change timing and keep a stale persisted render identity. The
    /// answer says whether the layout changed, which the host reports.
    pub(crate) fn invalidate_stale_freeze(&self, sample_rate: u32) -> bool {
        if !unsafe { hirari_track_frozen_store_is_frozen(self.frozen_store) }
            || unsafe { hirari_track_frozen_store_sample_rate(self.frozen_store) } == sample_rate
        {
            return false;
        }
        unsafe {
            hirari_track_frozen_store_clear(self.frozen_store);
            hirari_track_frozen_store_retire(self.frozen_store);
        }
        true
    }

    pub(crate) fn work_storage(&self) -> *mut c_void {
        self.work
    }

    fn window(&self, handle: *mut c_void, frames: u32) -> Option<Tap> {
        let (planes, count, capacity) = channels_of(handle);
        // A tap the finalise stage cannot address as stereo stays unpublished.
        if planes.is_null() || count < 2 || capacity < frames {
            return None;
        }
        let planes = unsafe { slice::from_raw_parts(planes, 2) };
        if planes[0].is_null() || planes[1].is_null() {
            return None;
        }
        Some(Tap {
            left: planes[0],
            right: planes[1],
        })
    }

    fn tap(&self, handle: *mut c_void, frames: u32) -> (*const *mut f32, u32) {
        let (planes, count, capacity) = channels_of(handle);
        // A tap the finalise stage cannot address as stereo stays unpublished.
        if count < 2 || capacity < frames {
            return (ptr::null(), 0);
        }
        (planes, count)
    }

    /// Runs one block: input gating, region rendering, MIDI, the insert chain
    /// and the output stage. A false return means the block was fully handled
    /// here (mute, freeze or an invalid block) and nothing downstream may run.
    unsafe fn process(&self, audio: *mut c_void, frames: u32, playhead: u64, clear: bool) -> bool {
        let (channels, channel_count, capacity) = channels_of(audio);
        if channels.is_null() || channel_count == 0 {
            return false;
        }
        if frames == 0 || frames > MAX_TRACK_BLOCK_FRAMES || frames > capacity {
            clear_planes(channels, channel_count, (frames as usize).min(capacity as usize));
            return false;
        }
        if self.muted.load(Ordering::Relaxed) {
            clear_planes(channels, channel_count, frames as usize);
            return false;
        }
        if unsafe { hirari_track_frozen_store_is_frozen(self.frozen_store) } {
            let view = unsafe { hirari_track_frozen_store_current(self.frozen_store) };
            let (sources, source_count, source_capacity) = channels_of(view.cast_mut());
            unsafe {
                hirari_audio_buffer_render_frozen(
                    channels,
                    channel_count,
                    capacity,
                    sources.cast(),
                    source_count,
                    source_capacity,
                    hirari_track_frozen_store_total_samples(self.frozen_store),
                    playhead,
                    frames,
                );
            }
            return false;
        }
        if clear {
            clear_planes(channels, channel_count, frames as usize);
        }
        let inputs = self
            .region_inputs
            .load(Ordering::Acquire)
            .cast::<crate::track_region_render::HirariTrackRegionRenderInput>();
        let region_count = self.region_count.load(Ordering::Acquire);
        if channel_count < 2
            || self.region_published.load(Ordering::Acquire) == 0
            || (region_count != 0 && inputs.is_null())
        {
            return false;
        }
        let planes = unsafe { slice::from_raw_parts(channels, channel_count as usize) };
        if planes[0].is_null() || planes[1].is_null() {
            return false;
        }
        unsafe {
            hirari_track_render_regions(
                inputs,
                region_count,
                playhead,
                frames,
                planes[0],
                planes[1],
                u8::from(self.phase_invert.load(Ordering::Relaxed)),
            );
        }

        let mut delay_count = 0usize;
        let delay_points = unsafe {
            hirari_track_automation_store_points(self.automation_store, 2, &mut delay_count)
        };
        let delay_hint = unsafe { hirari_track_automation_store_hint(self.automation_store, 2) };
        if !delay_points.is_null() && delay_count != 0 && !delay_hint.is_null() {
            let normalized = unsafe {
                hirari_realtime_automation_evaluate(delay_points.cast(), delay_count, playhead as f64, delay_hint)
            }
            .clamp(0.0, 1.0);
            let requested = (normalized * MAX_DELAY_SAMPLES as f32).round() as u32;
            self.track_delay_samples
                .store(requested.min(MAX_DELAY_SAMPLES), Ordering::Relaxed);
        }

        let mut event_count = 0usize;
        if self.transport_playing.load(Ordering::Relaxed) {
            let mut lane_count = 0usize;
            let lanes = unsafe {
                hirari_track_automation_store_plugin_lane_views(self.automation_store, &mut lane_count)
            };
            event_count = unsafe {
                hirari_plugin_automation_render_block(
                    lanes.cast(),
                    lane_count,
                    playhead,
                    frames,
                    self.plugin_events,
                    PLUGIN_EVENTS_PER_BLOCK,
                )
            };
        }

        self.prepare_midi_block(playhead, frames, planes[0], planes[1]);

        let (pre_insert, pre_insert_count, pre_insert_capacity) = channels_of(self.pre_insert);
        if pre_insert_count >= 2 && pre_insert_capacity >= frames && !pre_insert.is_null() {
            let destinations = unsafe { slice::from_raw_parts(pre_insert, pre_insert_count as usize) };
            if !destinations[0].is_null() && !destinations[1].is_null() {
                unsafe {
                    ptr::copy_nonoverlapping(planes[0], destinations[0], frames as usize);
                    ptr::copy_nonoverlapping(planes[1], destinations[1], frames as usize);
                }
            }
        }

        unsafe {
            hirari_effect_chain_parameter_automation(
                self.chain,
                self.plugin_events.cast::<TimedParameterEvent>().cast_const(),
                event_count,
            );
        }
        if channel_count >= 2 {
            unsafe {
                hirari_effect_chain_process(
                    self.chain,
                    audio,
                    self.midi,
                    f64::from_bits(self.transport_bpm.load(Ordering::Relaxed)),
                    self.sidechain_gate.load(Ordering::Acquire),
                    self.track_id,
                );
                // Processors share the mutable MIDI buffer for one block, so it
                // is dropped here rather than left for an alternate render path
                // to replay.
                hirari_midi_buffer_clear(self.midi);
            }
        }
        self.finalize(frames, playhead);
        true
    }

    /// Prepares one block's MIDI before the chain runs: expired expression
    /// note-offs are queued, the generated events are put in chronological
    /// order, and a track without an instrument gets its built-in voice. The
    /// scratch is bound at creation, so the audio thread never allocates.
    fn prepare_midi_block(&self, playhead: u64, frames: u32, left: *mut f32, right: *mut f32) {
        let capacity =
            unsafe { hirari_midi_buffer_remaining_capacity(self.midi) }.min(MIDI_EVENTS_PER_BLOCK);
        let count = unsafe {
            hirari_track_midi_flush_scheduled_note_offs(
                self.articulation,
                playhead,
                frames,
                if capacity == 0 {
                    ptr::null_mut()
                } else {
                    self.midi_events
                },
                capacity,
            )
        };
        for event in unsafe { slice::from_raw_parts(self.midi_events, count) } {
            unsafe {
                hirari_midi_buffer_add(
                    self.midi,
                    event.sample_offset,
                    event.data.as_ptr(),
                    u32::from(event.size),
                    event.articulation_id,
                );
            }
        }
        unsafe { hirari_midi_buffer_sort_owned(self.midi) };
        if !matches!(self.track_type, TRACK_TYPE_MIDI | TRACK_TYPE_INSTRUMENT)
            || unsafe { hirari_effect_chain_midi_instrument_count(self.chain) } != 0
            || frames == 0
            || left.is_null()
            || right.is_null()
        {
            return;
        }
        unsafe {
            hirari_builtin_midi_instrument_process(
                self.built_in_instrument,
                hirari_midi_buffer_event_data(self.midi),
                hirari_midi_buffer_event_count(self.midi),
                left,
                right,
                frames,
                hirari_channel_strip_sample_rate(self.channel_strip),
            )
        };
    }

    fn finalize(&self, frames: u32, playhead: u64) {
        let mut volume_count = 0usize;
        let mut pan_count = 0usize;
        let volume_points = unsafe {
            hirari_track_automation_store_points(self.automation_store, 0, &mut volume_count)
                .cast()
        };
        let pan_points = unsafe {
            hirari_track_automation_store_points(self.automation_store, 1, &mut pan_count).cast()
        };
        let (active, active_count) = self.tap(self.work, frames);
        if active.is_null() {
            return;
        }
        let (pre_insert, pre_insert_count) = self.tap(self.pre_insert, frames);
        let (send_pre, send_pre_count) = self.tap(self.send_pre, frames);
        let (send_post, send_post_count) = self.tap(self.send_post, frames);
        let (pre_fader, pre_fader_count) = self.tap(self.pre_fader, frames);
        let mut low_state_left = read(&self.low_state_left);
        let mut low_state_right = read(&self.low_state_right);
        unsafe {
            finalize_track_output(&TrackFinalizeInput {
                track_id: self.track_id,
                frames,
                start_time: playhead as f64,
                volume_points,
                volume_count,
                volume_hint: hirari_track_automation_store_hint(self.automation_store, 0),
                pan_points,
                pan_count,
                pan_hint: hirari_track_automation_store_hint(self.automation_store, 1),
                channel_strip: self.channel_strip,
                active_channels: active,
                active_channel_count: active_count,
                pre_insert_channels: pre_insert,
                pre_insert_channel_count: pre_insert_count,
                send_pre_channels: send_pre,
                send_pre_channel_count: send_pre_count,
                send_post_channels: send_post,
                send_post_channel_count: send_post_count,
                panner: self.panner,
                spatial_mode: 0,
                spatial_x: read(&self.spatial_x),
                spatial_y: read(&self.spatial_y),
                spatial_z: read(&self.spatial_z),
                low_gain: read(&self.low_gain),
                high_gain: read(&self.high_gain),
                low_state_l: &mut low_state_left,
                low_state_r: &mut low_state_right,
                pdc_delay: self.pdc_delay,
                requested_pdc: self.pdc_requested.load(Ordering::Acquire),
                manual_delay: self.track_delay_samples.load(Ordering::Acquire),
                pre_fader_channels: pre_fader,
                pre_fader_channel_count: pre_fader_count,
            });
        }
        self.low_state_left
            .store(low_state_left.to_bits(), Ordering::Relaxed);
        self.low_state_right
            .store(low_state_right.to_bits(), Ordering::Relaxed);
    }
}

pub(crate) fn runtime(state: *mut c_void) -> Option<&'static TrackRuntime> {
    unsafe { state.cast::<TrackRuntime>().as_ref() }
}

/// Binds one track's realtime state. Every handle must outlive the returned
/// runtime, which the track destroys with itself.
///
/// # Safety
/// The handles belong to this one track and stay alive for the runtime's life.
#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub unsafe extern "C" fn hirari_track_runtime_create(
    track_id: u32,
    track_type: u32,
    chain: *mut c_void,
    channel_strip: *mut c_void,
    automation_store: *mut c_void,
    frozen_store: *mut c_void,
    panner: *mut c_void,
    pdc_delay: *mut c_void,
    work: *mut c_void,
    pre_insert: *mut c_void,
    pre_fader: *mut c_void,
    send_pre: *mut c_void,
    send_post: *mut c_void,
    midi: *mut c_void,
    articulation: *mut c_void,
    built_in_instrument: *mut c_void,
) -> *mut c_void {
    let mut scratch = vec![NativeTimedParameterEvent::default(); PLUGIN_EVENTS_PER_BLOCK];
    let plugin_events = scratch.as_mut_ptr();
    // The runtime owns the scratch from here; destroy rebuilds the Vec so the
    // block pipeline can write through a stable pointer with no per-block alloc.
    std::mem::forget(scratch);
    let mut midi_scratch = vec![GeneratedMidiEvent::default(); MIDI_EVENTS_PER_BLOCK];
    let midi_events = midi_scratch.as_mut_ptr();
    std::mem::forget(midi_scratch);
    Box::into_raw(Box::new(TrackRuntime {
        track_id,
        track_type,
        chain,
        channel_strip,
        automation_store,
        frozen_store,
        panner,
        pdc_delay,
        work,
        pre_insert,
        pre_fader,
        send_pre,
        send_post,
        midi,
        articulation,
        built_in_instrument,
        midi_events,
        plugin_events,
        muted: AtomicBool::new(false),
        phase_invert: AtomicBool::new(false),
        transport_playing: AtomicBool::new(false),
        transport_bpm: AtomicU64::new(120.0f64.to_bits()),
        pdc_requested: AtomicU32::new(0),
        track_delay_samples: AtomicU32::new(0),
        spatial_x: stored(0.0),
        spatial_y: stored(0.0),
        spatial_z: stored(0.0),
        low_gain: stored(1.0),
        high_gain: stored(1.0),
        low_state_left: stored(0.0),
        low_state_right: stored(0.0),
        region_inputs: AtomicPtr::new(ptr::null_mut()),
        region_count: AtomicUsize::new(0),
        region_published: AtomicU8::new(0),
        sidechain_gate: AtomicPtr::new(ptr::null_mut()),
    }))
    .cast()
}

/// # Safety
/// `state` is null or a runtime returned by `hirari_track_runtime_create`.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_runtime_destroy(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    let runtime = unsafe { Box::from_raw(state.cast::<TrackRuntime>()) };
    // The scratch was leaked out of a Vec so the block pipeline could write it
    // through the shared track handle.
    drop(unsafe {
        Vec::from_raw_parts(runtime.plugin_events, PLUGIN_EVENTS_PER_BLOCK, PLUGIN_EVENTS_PER_BLOCK)
    });
    drop(unsafe {
        Vec::from_raw_parts(runtime.midi_events, MIDI_EVENTS_PER_BLOCK, MIDI_EVENTS_PER_BLOCK)
    });
}

/// Renders one audio block for a track.
///
/// # Safety
/// `state` is a live runtime and `audio` is a live Rust audio storage with the
/// frame count the track was prepared for.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_process_block(
    state: *mut c_void,
    audio: *mut c_void,
    frames: u32,
    playhead: u64,
    clear_input: u8,
) -> bool {
    let Some(runtime) = runtime(state) else {
        return false;
    };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        unsafe { runtime.process(audio, frames, playhead, clear_input != 0) }
    }))
    .unwrap_or(false)
}

macro_rules! flag {
    ($name:ident, $field:ident) => {
        /// # Safety
        /// `state` is a live runtime.
        #[no_mangle]
        pub unsafe extern "C" fn $name(state: *mut c_void, value: u8) {
            if let Some(runtime) = runtime(state) {
                runtime.$field.store(value != 0, Ordering::Relaxed);
            }
        }
    };
}

flag!(hirari_track_runtime_set_muted, muted);
flag!(hirari_track_runtime_set_phase_invert, phase_invert);

macro_rules! is_flag {
    ($name:ident, $field:ident) => {
        /// # Safety
        /// `state` is a live runtime.
        #[no_mangle]
        pub unsafe extern "C" fn $name(state: *mut c_void) -> bool {
            runtime(state).is_some_and(|runtime| runtime.$field.load(Ordering::Relaxed))
        }
    };
}

is_flag!(hirari_track_runtime_is_muted, muted);
is_flag!(hirari_track_runtime_is_phase_inverted, phase_invert);

/// # Safety
/// `state` is a live runtime.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_runtime_transport_bpm(state: *mut c_void) -> f64 {
    runtime(state)
        .map_or(120.0, |runtime| {
            f64::from_bits(runtime.transport_bpm.load(Ordering::Relaxed))
        })
}

/// Publishes the tempo and playback flag for one block. The engine clamps the
/// tempo before it gets here, so the realtime value is taken as-is.
///
/// # Safety
/// `state` is a live runtime.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_runtime_set_transport(state: *mut c_void, bpm: f64, playing: u8) {
    if let Some(runtime) = runtime(state) {
        runtime.transport_bpm.store(bpm.to_bits(), Ordering::Relaxed);
        runtime
            .transport_playing
            .store(playing != 0, Ordering::Relaxed);
    }
}

/// Publishes the manual timing offset and the plugin latency the graph resolved
/// for this track; both are bounded by the shared delay line.
///
/// # Safety
/// `state` is a live runtime.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_runtime_set_delays(
    state: *mut c_void,
    pdc_requested: u32,
    manual_delay: u32,
) {
    if let Some(runtime) = runtime(state) {
        runtime
            .pdc_requested
            .store(pdc_requested.min(MAX_DELAY_SAMPLES), Ordering::Relaxed);
        runtime
            .track_delay_samples
            .store(manual_delay.min(MAX_DELAY_SAMPLES), Ordering::Relaxed);
    }
}

/// # Safety
/// `state` is a live runtime.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_runtime_pdc_requested(state: *mut c_void) -> u32 {
    runtime(state).map_or(0, |runtime| {
        runtime.pdc_requested.load(Ordering::Acquire)
    })
}

/// # Safety
/// `state` is a live runtime.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_runtime_track_delay_samples(state: *mut c_void) -> u32 {
    runtime(state).map_or(0, |runtime| {
        runtime.track_delay_samples.load(Ordering::Acquire)
    })
}

/// Publishes the spatial coordinates the track's panner stage would use.
///
/// # Safety
/// `state` is a live runtime.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_runtime_set_spatial(
    state: *mut c_void,
    x: f32,
    y: f32,
    z: f32,
) {
    if let Some(runtime) = runtime(state) {
        runtime.spatial_x.store(x.to_bits(), Ordering::Relaxed);
        runtime.spatial_y.store(y.to_bits(), Ordering::Relaxed);
        runtime.spatial_z.store(z.to_bits(), Ordering::Relaxed);
    }
}

/// # Safety
/// `state` is a live runtime.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_runtime_spatial(state: *mut c_void, axis: u32) -> f32 {
    runtime(state).map_or(0.0, |runtime| {
        match axis {
            0 => read(&runtime.spatial_x),
            1 => read(&runtime.spatial_y),
            _ => read(&runtime.spatial_z),
        }
    })
}

/// Publishes the track's fixed shelf gains. The filter memory lives here, so no
/// C++ pointer escapes into the realtime stage.
///
/// # Safety
/// `state` is a live runtime.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_runtime_set_eq(state: *mut c_void, low: f32, high: f32) {
    if let Some(runtime) = runtime(state) {
        runtime.low_gain.store(low.to_bits(), Ordering::Relaxed);
        runtime.high_gain.store(high.to_bits(), Ordering::Relaxed);
    }
}

/// # Safety
/// `state` is a live runtime.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_runtime_eq(state: *mut c_void, side: u32) -> f32 {
    runtime(state).map_or(1.0, |runtime| {
        if side == 0 {
            read(&runtime.low_gain)
        } else {
            read(&runtime.high_gain)
        }
    })
}

/// Points the audio thread at a rendered, immutable region table.
///
/// # Safety
/// `state` is a live runtime and `inputs` stays readable until the next
/// publication, which the region store guarantees by retiring it late.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_runtime_publish_regions(
    state: *mut c_void,
    inputs: *const u8,
    count: usize,
    published: u8,
) {
    if let Some(runtime) = runtime(state) {
        runtime.region_inputs.store(inputs.cast_mut().cast(), Ordering::Release);
        runtime.region_count.store(count, Ordering::Release);
        runtime
            .region_published
            .store(published, Ordering::Release);
    }
}

/// Re-arms the track for an offline render: the smoothed strip state, the chain,
/// the automation scan hints and the delay line all restart from zero.
///
/// # Safety
/// `state` is a live runtime.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_runtime_reset_for_offline_render(state: *mut c_void) {
    let Some(runtime) = runtime(state) else {
        return;
    };
    let requested = runtime.pdc_requested.load(Ordering::Acquire);
    let manual = runtime.track_delay_samples.load(Ordering::Acquire).min(MAX_DELAY_SAMPLES);
    let bounded = if requested > MAX_DELAY_SAMPLES - manual {
        MAX_DELAY_SAMPLES
    } else {
        requested + manual
    };
    unsafe {
        hirari_channel_strip_reset(runtime.channel_strip);
        hirari_effect_chain_reset(runtime.chain);
        hirari_track_automation_store_reset_hints(runtime.automation_store);
        hirari_track_pdc_delay_reset(runtime.pdc_delay, bounded);
    }
}

/// Why one stereo block of a track could not be grown to a new device format.
/// A growth the realtime guard refused leaves the previous size in place, so
/// only an impossible size or an exhausted allocation is a failure the host has
/// to report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackPrepare {
    Ready,
    Declined,
    InvalidSize,
    OutOfMemory,
}

impl TrackPrepare {
    const fn status(self) -> u8 {
        match self {
            Self::Ready => 0,
            Self::Declined => 1,
            Self::InvalidSize => 2,
            Self::OutOfMemory => 3,
        }
    }

    /// The `reserve_buffer` status of one block growth, read as this enum.
    const fn from_reserve(status: u8) -> Self {
        match status {
            0 => Self::Ready,
            1 | 5 => Self::Declined,
            2 => Self::InvalidSize,
            _ => Self::OutOfMemory,
        }
    }
}

/// Rebuilds a track for a new device format: the region store learns the rate
/// and block length, the strip and the panner retune, the smoothed strip state
/// snaps to its targets, every block the track renders through grows to the new
/// length, and the plug-in chain is prepared last so it sees the finished
/// buffers. The host keeps the allocation policy: a refused growth is silent, an
/// impossible size or an exhausted allocation is its own failure.
///
/// # Safety
/// `state` is a live runtime, and `region_key` is the handle the region store
/// knows this track by, or null.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_runtime_prepare(
    state: *mut c_void,
    region_key: *mut c_void,
    sample_rate: f64,
    block_frames: u32,
) -> u8 {
    let Some(runtime) = runtime(state) else {
        return TrackPrepare::Ready.status();
    };
    runtime.prepare(region_key, sample_rate, block_frames)
}

#[cfg(test)]
#[path = "track_runtime_tests.rs"]
mod tests;

