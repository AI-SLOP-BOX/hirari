//! Audio-region timing jobs: align-to-reference and sync-group quantize.
//!
//! The analysis kernels already live in Rust (`region_alignment`,
//! `audio_quantizer`). This module owns everything around them: request
//! validation, the per-reference job registry, conflict rejection, worker
//! threads, staleness checks, the marker commit and its undo entry. The native
//! engine only retains immutable audio buffers, applies marker lists and
//! records the undo pair.
//!
//! Every entry point runs on the control thread. Worker threads only read
//! retained audio and then publish their marker lists back into the job.

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::audio_quantizer::{
    hirari_audio_quantizer_build_group_warp_maps, hirari_audio_quantizer_group_warp_map_copy,
    hirari_audio_quantizer_group_warp_map_count, hirari_audio_quantizer_group_warp_maps_destroy,
    hirari_audio_quantizer_settings_validate, HirariAudioQuantizeChannelView,
    HirariAudioQuantizerTargetView, HirariAudioQuantizerTempoEvent,
};
use crate::region_alignment::{
    hirari_region_align_audio, hirari_region_alignment_target_ids_valid,
    HirariRegionAlignmentTargetId,
};
use crate::region_edit_host::{
    host_ops, record, warp_markers_of, HirariRegionEditPayload, HirariRegionHostOps,
    HirariRegionState, HIRARI_REGION_EDIT_MARKERS,
};
use crate::region_warp::HirariWarpMarker;
use crate::tempo::{
    hirari_tempo_map_copy_events, hirari_tempo_map_get_current_bpm,
    hirari_tempo_map_get_event_count,
};
use crate::undo_transaction::hirari_undo_manager_transaction_active;

const STATUS_ANALYZING: u8 = 1;
const STATUS_READY: u8 = 2;
const STATUS_APPLIED: u8 = 3;
const STATUS_FAILED: u8 = 4;

const MAX_ALIGNMENT_TARGETS: usize = 32;
const MIN_ALIGNMENT_SPAN: u64 = 1024;
const MAX_ALIGNMENT_SPAN: u64 = 16 * 1024 * 1024;
const MAX_QUANTIZE_SPAN: u64 = 16_777_216;
const MAX_CHANNELS: u32 = 32;
const MAX_ALIGNMENT_MARKERS: usize = 257;

fn region_span(state: &HirariRegionState) -> u64 {
    if state.source_length > 0 {
        state.source_length
    } else {
        state.len
    }
}

/// A retained, immutable view of a region's decoded audio. The pointers stay
/// valid while the owning `RetainedAudio` lives, because the host keeps the
/// decoded buffer alive through the handle.
struct AudioView {
    channels: Vec<*const f32>,
    samples: u64,
    channel_count: u32,
}

struct RetainedAudio {
    ops: HirariRegionHostOps,
    handle: *mut c_void,
}

// The handle points at host-owned immutable audio; the worker only reads it.
unsafe impl Send for RetainedAudio {}
unsafe impl Sync for RetainedAudio {}

impl RetainedAudio {
    fn retain(
        ops: &HirariRegionHostOps,
        host: *mut c_void,
        track_id: u32,
        region_id: u32,
    ) -> Option<Self> {
        let retain = ops.retain_audio?;
        let handle = unsafe { retain(host, track_id, region_id) };
        if handle.is_null() {
            return None;
        }
        Some(Self { ops: *ops, handle })
    }

    fn view(&self) -> Option<AudioView> {
        let view = self.ops.audio_view?;
        let mut samples = 0_u64;
        let channel_count = unsafe { view(self.handle, ptr::null_mut(), 0, &mut samples) };
        if channel_count == 0 || channel_count > MAX_CHANNELS || samples == 0 {
            return None;
        }
        let mut channels = vec![ptr::null(); channel_count as usize];
        let written = unsafe {
            view(
                self.handle,
                channels.as_mut_ptr(),
                channels.len(),
                &mut samples,
            )
        };
        if written != channel_count || channels.iter().any(|channel| channel.is_null()) {
            return None;
        }
        Some(AudioView {
            channels,
            samples,
            channel_count,
        })
    }
}

impl Drop for RetainedAudio {
    fn drop(&mut self) {
        if let Some(release) = self.ops.release_audio {
            unsafe { release(self.handle) };
        }
    }
}

/// One region captured for a job: the state and undo markers are snapshots,
#[path = "region_time_jobs_execute.rs"]
mod execute;
pub(in crate::region_time_jobs) use execute::*;

#[no_mangle]
pub unsafe extern "C" fn hirari_region_align_to_reference(
    host: *mut c_void,
    reference_track: u32,
    reference_region: u32,
    targets: *const HirariRegionAlignmentTargetId,
    target_count: usize,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    if target_count != 0 && targets.is_null() {
        return false;
    }
    let targets: Vec<(u32, u32)> = if target_count == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(targets, target_count) }
            .iter()
            .map(|target| (target.track_id, target.region_id))
            .collect()
    };
    align_with(&ops, host, reference_track, reference_region, &targets)
}

/// # Safety
/// `host` must be the handle the installed region host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_alignment_status(
    host: *mut c_void,
    reference_track: u32,
    reference_region: u32,
) -> u8 {
    status_of(
        alignment_registry(),
        host,
        region_key(reference_track, reference_region),
    )
}

/// # Safety
/// `host` must be the handle the installed region host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_finalize_alignment(
    host: *mut c_void,
    reference_track: u32,
    reference_region: u32,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    align_finalize(&ops, host, reference_track, reference_region)
}

/// Starts a synchronized quantize. Returns false when the settings, the sync
/// group or any member region would make the result ambiguous.
///
/// # Safety
/// `host` must be the handle the installed region host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_quantize_sync_group(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    strength: f32,
    grid_beats: f64,
    swing: f32,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    quantize_with(&ops, host, track_id, region_id, strength, grid_beats, swing)
}

/// # Safety
/// `host` must be the handle the installed region host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_quantize_status(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> u8 {
    status_of(quantize_registry(), host, region_key(track_id, region_id))
}

/// # Safety
/// `host` must be the handle the installed region host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_finalize_quantize(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    quantize_finalize(&ops, host, track_id, region_id)
}


#[cfg(test)]
#[path = "region_time_jobs_tests.rs"]
mod region_time_jobs_tests;

