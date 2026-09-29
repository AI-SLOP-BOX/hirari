use crate::realtime_automation::{
    hirari_plugin_automation_render_block, hirari_realtime_automation_evaluate,
    NativeAutomationPoint, NativePluginAutomationLaneView, NativeTimedParameterEvent,
};
use crate::track_region_render::{hirari_track_render_regions, HirariTrackRegionRenderInput};
use std::ffi::c_void;
use std::ptr;
use std::slice;

const MAX_TRACK_BLOCK_FRAMES: u32 = 65_536;
const MAX_TRACK_FREEZE_SAMPLES: u64 = 64 * 1024 * 1024;

pub type TrackMidiPrepare = unsafe extern "C" fn(*mut c_void, u64, u32);
pub type TrackFreezeReset = unsafe extern "C" fn(*mut c_void) -> bool;
pub type TrackFreezeProcess = unsafe extern "C" fn(
    *mut c_void,
    *mut f32,
    *mut f32,
    u32,
    u64,
) -> bool;

/// Owns the bounded offline freeze render loop. Native callbacks only reset
/// and render one track block into caller-provided scratch storage.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_freeze_render(
    total_samples: u64,
    block_size: u32,
    output_left: *mut f32,
    output_right: *mut f32,
    output_capacity: usize,
    context: *mut c_void,
    reset: Option<TrackFreezeReset>,
    process: Option<TrackFreezeProcess>,
) -> bool {
    let render = || {
        let (Some(reset), Some(process)) = (reset, process) else {
            return false;
        };
        let Ok(total_samples) = usize::try_from(total_samples) else {
            return false;
        };
        if total_samples == 0
            || total_samples as u64 > MAX_TRACK_FREEZE_SAMPLES
            || total_samples > output_capacity
            || output_left.is_null()
            || output_right.is_null()
            || block_size == 0
            || block_size > MAX_TRACK_BLOCK_FRAMES
            || context.is_null()
        {
            return false;
        }
        let Some(output_bytes) = total_samples.checked_mul(std::mem::size_of::<f32>()) else {
            return false;
        };
        let left_start = output_left as usize;
        let right_start = output_right as usize;
        let (Some(left_end), Some(right_end)) = (
            left_start.checked_add(output_bytes),
            right_start.checked_add(output_bytes),
        ) else {
            return false;
        };
        if left_start < right_end && right_start < left_end {
            return false;
        }
        let mut left_block = vec![0.0f32; block_size as usize];
        let mut right_block = vec![0.0f32; block_size as usize];
        if !unsafe { reset(context) } {
            return false;
        }
        let left = unsafe { slice::from_raw_parts_mut(output_left, total_samples) };
        let right = unsafe { slice::from_raw_parts_mut(output_right, total_samples) };
        let mut offset = 0usize;
        while offset < total_samples {
            let frames = (total_samples - offset).min(block_size as usize);
            if !unsafe {
                process(
                    context,
                    left_block.as_mut_ptr(),
                    right_block.as_mut_ptr(),
                    frames as u32,
                    offset as u64,
                )
            } {
                return false;
            }
            left[offset..offset + frames].copy_from_slice(&left_block[..frames]);
            right[offset..offset + frames].copy_from_slice(&right_block[..frames]);
            offset += frames;
        }
        true
    };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(render)).unwrap_or(false)
}

/// All pre-insert realtime work for one track block. Storage and snapshots are
/// owned by Track; Rust coordinates the bounded block pipeline without taking
/// ownership or allocating on the audio thread.
#[repr(C)]
pub struct TrackProcessRequest {
    pub channels: *const *mut f32,
    pub channel_count: u32,
    pub buffer_capacity: u32,
    pub frames: u32,
    pub playhead: u64,
    pub clear_input: u8,
    pub muted: u8,
    pub frozen: u8,
    pub frozen_channels: *const *const f32,
    pub frozen_channel_count: u32,
    pub frozen_capacity: u32,
    pub frozen_total_samples: u64,
    pub region_inputs: *const HirariTrackRegionRenderInput,
    pub region_count: usize,
    pub region_snapshot_available: u8,
    pub phase_invert: u8,
    pub track_delay_points: *const NativeAutomationPoint,
    pub track_delay_count: usize,
    pub track_delay_hint: *mut usize,
    pub track_delay_output: *mut u32,
    pub max_track_delay_samples: u32,
    pub plugin_lanes: *const NativePluginAutomationLaneView,
    pub plugin_lane_count: usize,
    pub transport_playing: u8,
    pub plugin_events: *mut NativeTimedParameterEvent,
    pub plugin_event_capacity: usize,
    pub plugin_event_count_out: *mut usize,
    pub pre_insert_channels: *const *mut f32,
    pub pre_insert_channel_count: u32,
    pub pre_insert_capacity: u32,
    pub midi_prepare: Option<TrackMidiPrepare>,
    pub midi_context: *mut c_void,
}

unsafe fn clear_channels(channels: *const *mut f32, count: u32, frames: usize) {
    if channels.is_null() || frames == 0 {
        return;
    }
    let pointers = unsafe { slice::from_raw_parts(channels, count as usize) };
    for &channel in pointers {
        if !channel.is_null() {
            unsafe { ptr::write_bytes(channel, 0, frames) };
        }
    }
}

/// Runs the track pipeline through region rendering and MIDI preparation.
/// Returns true only when the caller should continue into the native effect
/// chain; mute/freeze/invalid blocks are fully handled and return false.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_process_prepare(request: *const TrackProcessRequest) -> bool {
    let Some(request) = (unsafe { request.as_ref() }) else {
        return false;
    };
    if request.plugin_event_count_out.is_null() {
        return false;
    }
    unsafe { request.plugin_event_count_out.write(0) };

    let frames = request.frames;
    let safe_frames = frames.min(request.buffer_capacity) as usize;
    if request.channels.is_null() || request.channel_count == 0 {
        return false;
    }
    if frames == 0 || frames > MAX_TRACK_BLOCK_FRAMES || frames > request.buffer_capacity {
        unsafe { clear_channels(request.channels, request.channel_count, safe_frames) };
        return false;
    }
    if request.muted != 0 {
        unsafe { clear_channels(request.channels, request.channel_count, frames as usize) };
        return false;
    }
    if request.frozen != 0 {
        unsafe {
            crate::audio_buffer_ops::hirari_audio_buffer_render_frozen(
                request.channels.cast_mut(),
                request.channel_count,
                request.buffer_capacity,
                request.frozen_channels,
                request.frozen_channel_count,
                request.frozen_capacity,
                request.frozen_total_samples,
                request.playhead,
                frames,
            );
        }
        return false;
    }
    if request.clear_input != 0 {
        unsafe { clear_channels(request.channels, request.channel_count, frames as usize) };
    }
    if request.channel_count < 2
        || request.region_snapshot_available == 0
        || (request.region_count != 0 && request.region_inputs.is_null())
    {
        return false;
    }
    let channels =
        unsafe { slice::from_raw_parts(request.channels, request.channel_count as usize) };
    if channels[0].is_null() || channels[1].is_null() {
        return false;
    }
    unsafe {
        hirari_track_render_regions(
            request.region_inputs,
            request.region_count,
            request.playhead,
            frames,
            channels[0],
            channels[1],
            request.phase_invert,
        );
    }

    if !request.track_delay_points.is_null()
        && request.track_delay_count != 0
        && !request.track_delay_hint.is_null()
        && !request.track_delay_output.is_null()
    {
        let normalized = unsafe {
            hirari_realtime_automation_evaluate(
                request.track_delay_points,
                request.track_delay_count,
                request.playhead as f64,
                request.track_delay_hint,
            )
        }
        .clamp(0.0, 1.0);
        let requested = (normalized * request.max_track_delay_samples as f32).round() as u32;
        unsafe {
            request
                .track_delay_output
                .write(requested.min(request.max_track_delay_samples))
        };
    }

    if request.transport_playing != 0 {
        let event_count = unsafe {
            hirari_plugin_automation_render_block(
                request.plugin_lanes,
                request.plugin_lane_count,
                request.playhead,
                frames,
                request.plugin_events,
                request.plugin_event_capacity,
            )
        };
        unsafe { request.plugin_event_count_out.write(event_count) };
    }

    if let Some(prepare_midi) = request.midi_prepare {
        unsafe { prepare_midi(request.midi_context, request.playhead, frames) };
    } else {
        return false;
    }

    if request.pre_insert_channel_count >= 2
        && request.pre_insert_capacity >= frames
        && !request.pre_insert_channels.is_null()
    {
        let destinations = unsafe {
            slice::from_raw_parts(
                request.pre_insert_channels,
                request.pre_insert_channel_count as usize,
            )
        };
        if !destinations[0].is_null() && !destinations[1].is_null() {
            unsafe {
                ptr::copy_nonoverlapping(channels[0], destinations[0], frames as usize);
                ptr::copy_nonoverlapping(channels[1], destinations[1], frames as usize);
            }
        }
    }
    true
}
