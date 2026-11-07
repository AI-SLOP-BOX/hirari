//! Event, automation mode, and track-buffer audio operations moved from C++
//! (`hirari_unified_engine_part_7.inc`).
//!
//! Every store backing these operations — the engine event ring, the engine block,
//! the track table, and the track work buffers — lives in Rust and is accessed
//! through the host address and `TrackSession`.

use std::ffi::c_void;

use crate::audio_buffer_ops::{
    hirari_audio_buffer_storage_channels, hirari_audio_buffer_storage_read_pointer,
    hirari_audio_buffer_storage_samples, hirari_audio_buffer_storage_write_pointer,
};
use crate::automation_capture_runtime::hirari_automation_record_mode_apply;
use crate::engine_block_host::hirari_engine_block_set_auto_record_mode;
use crate::engine_rings::hirari_engine_event_ring_pop;
use crate::engine_session::{handle_of_host, H_ENGINE_BLOCK, H_ENGINE_RINGS};
use crate::ffi::{BridgeEvent};
use crate::AudioEngine;
use crate::HirariUnifiedEngine;
use crate::stereo_dsp::{hirari_stereo_correlation, hirari_vocal_remove_stereo};
use crate::track_session::TrackSession;

#[inline]
pub fn host_from_core(core: &HirariUnifiedEngine) -> *mut c_void {
    core as *const HirariUnifiedEngine as *mut c_void
}

pub fn set_automation_record_mode(host: *mut c_void, mode: u32) -> bool {
    let block = handle_of_host(host, H_ENGINE_BLOCK);
    if block.is_null() {
        return false;
    }
    unsafe {
        hirari_engine_block_set_auto_record_mode(block, hirari_automation_record_mode_apply(mode));
    }
    true
}

pub fn execute_vocal_remover(host: *mut c_void, tid: u32) -> bool {
    let Some(session) = TrackSession::open(host, tid) else {
        return false;
    };
    let buffer = session.work_buffer();
    if buffer.is_null() {
        return false;
    }
    let channels = unsafe { hirari_audio_buffer_storage_channels(buffer) };
    if channels < 2 {
        return false;
    }
    let samples = unsafe { hirari_audio_buffer_storage_samples(buffer) } as usize;
    let left = unsafe { hirari_audio_buffer_storage_write_pointer(buffer, 0) };
    let right = unsafe { hirari_audio_buffer_storage_write_pointer(buffer, 1) };
    if left.is_null() || right.is_null() {
        return false;
    }
    unsafe { hirari_vocal_remove_stereo(left, right, samples) }
}

pub fn get_track_correlation(host: *mut c_void, tid: u32) -> f32 {
    let Some(session) = TrackSession::open(host, tid) else {
        return 1.0;
    };
    let buffer = session.work_buffer();
    if buffer.is_null() {
        return 1.0;
    }
    let channels = unsafe { hirari_audio_buffer_storage_channels(buffer) };
    if channels < 2 {
        return 1.0;
    }
    let samples = unsafe { hirari_audio_buffer_storage_samples(buffer) } as usize;
    let left = unsafe { hirari_audio_buffer_storage_read_pointer(buffer, 0, 0) };
    let right = unsafe { hirari_audio_buffer_storage_read_pointer(buffer, 1, 0) };
    if left.is_null() || right.is_null() {
        return 1.0;
    }
    unsafe { hirari_stereo_correlation(left, right, samples) }
}

pub fn pop_event(host: *mut c_void, event: &mut BridgeEvent) -> bool {
    let rings = handle_of_host(host, H_ENGINE_RINGS);
    if rings.is_null() {
        return false;
    }
    unsafe {
        hirari_engine_event_ring_pop(
            rings.cast(),
            &mut event.timestamp,
            &mut event.event_type,
            &mut event.track_id,
            &mut event.value,
            event.label.as_mut_ptr(),
            event.label.len(),
        )
    }
}
