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

#[path = "track_runtime_impl.rs"]
mod runtime_impl;

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

