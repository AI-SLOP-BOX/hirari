//! The engine's region table, owned by Rust.
//!
//! A track's regions — editable state, source path and name, the decoded-audio
//! handle, the clip-edit/note/processing tables, the spectral stretcher and the
//! immutable projection the audio callback reads — live here. The C++ `Track`
//! keeps only the published render-input array, its length and a published
//! flag, so the realtime callback never touches a region object.
//!
//! C++ supplies the host table below: it answers for the decoded buffer it
//! owns, installs a published array into the track's atomics, invalidates the
//! rendered freeze and sets the layout dirty bit. Region normalization on
//! insert, edit storage, snapshot publication and retirement are decided here.
//!
//! Locking: the store lock is the innermost lock. A host callback may run while
//! it is held, so the host side must not take the project topology lock in the
//! callbacks below.

use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::ffi::c_void;
use std::ptr;
use std::slice;
use std::sync::{Arc, Mutex, OnceLock};

use crate::audio_pitch_analysis::{
    hirari_audio_pitch_commit_region_result, hirari_audio_pitch_result_destroy,
};
use crate::region_audio::hirari_region_waveform_forget;
use crate::region_clip_edits::{
    hirari_region_clip_edits_clone, hirari_region_clip_edits_create,
    hirari_region_clip_edits_destroy, hirari_region_clip_edits_freeze,
    hirari_region_clip_edits_normalize_range_edits,
    hirari_region_clip_edits_normalize_warp_markers, hirari_region_clip_edits_replace_comp_ranges,
    hirari_region_clip_edits_replace_range_edits, hirari_region_clip_edits_set_warp_markers,
    RegionClipEdits, RegionClipEditsFrozen, RegionClipEditsView,
};
use crate::region_comp_ranges::NativeCompRange;
use crate::region_edit_host::{
    hirari_region_note_store_restore, HirariRegionEditPayload, HirariRegionGroupMember,
    HirariRegionNoteEditFn, HirariRegionState, HIRARI_REGION_EDIT_COMP_RANGES,
    HIRARI_REGION_EDIT_GEOMETRY, HIRARI_REGION_EDIT_MARKERS, HIRARI_REGION_EDIT_NOTE_STORE,
    HIRARI_REGION_EDIT_PITCH_COMMIT, HIRARI_REGION_EDIT_RANGE_EDITS, HIRARI_REGION_EDIT_REMOVE,
    HIRARI_REGION_EDIT_REPLACE_AUDIO, HIRARI_REGION_EDIT_RESTORE, HIRARI_REGION_EDIT_STATE,
};
use crate::region_processing::{hirari_region_normalize_add_metadata, NativeRangeEdit};
use crate::region_processing_history::{
    hirari_region_processing_history_append, hirari_region_processing_history_clear,
    hirari_region_processing_history_count, hirari_region_processing_history_materialize,
    hirari_region_processing_history_new, hirari_region_processing_history_reset,
    hirari_region_processing_history_sizes, HirariEventStep, HirariEventStepView,
    HirariRegionProcessingHistoryStore,
};
use crate::region_resampler::hirari_region_resampler_prepare;
use crate::region_time_stretch::{
    hirari_region_stretch_create, hirari_region_stretch_destroy, hirari_region_stretch_prepare,
};
use crate::region_warp::HirariWarpMarker;
use crate::track_region_render::HirariTrackRegionRenderInput;
use crate::track_region_snapshot::{
    hirari_track_build_region_render_inputs, HirariRegionRenderSource,
};
use crate::vari_audio::{
    hirari_region_vari_audio_freeze, hirari_region_vari_audio_frozen_curves,
    hirari_region_vari_audio_frozen_ranges, hirari_region_vari_audio_store_clear,
    hirari_region_vari_audio_store_clone, hirari_region_vari_audio_store_new,
    hirari_region_vari_audio_store_normalize, HirariRegionVariAudioStore, HirariVariAudioFrozen,
};

/// Matches the C++ WSOLA window the renderer was built with: 1024 raised-cosine
/// taps, constructed once before any callback can run.
const WSOLA_WINDOW_TAPS: usize = 1024;

#[path = "region_store_types_setup.rs"]
mod types_setup;
#[path = "region_store_publish.rs"]
mod publish_;
#[path = "region_store_read.rs"]
mod read_;
#[path = "region_store_insert.rs"]
mod insert_;
pub(crate) use types_setup::*;
pub(crate) use publish_::*;
pub(crate) use read_::*;
pub(crate) use insert_::*;
#[path = "region_store_ffi.rs"]
mod store_ffi;
pub(crate) use store_ffi::*;

fn apply_one(track: &mut TrackRegions, payload: &HirariRegionEditPayload) -> bool {

    invalidate_frozen(track.host);

    match payload.op {

        HIRARI_REGION_EDIT_STATE => {

            let Some(index) = entry_index(track, payload.region_id) else {

                return false;

            };

            let mut snapshot = match region_snapshot(track, payload.region_id) {

                Some(snapshot) => snapshot,

                None => return false,

            };

            snapshot.state = payload.state;

            // The id is stable, so the replacement takes the removed region's

            // place after the same sorted-insert normalization `addRegion` ran.

            track.entries.remove(index);

            if !add_snapshot(track, payload.region_id, &snapshot) {

                return false;

            }

            publish(track);

            true

        }

        HIRARI_REGION_EDIT_REMOVE => {

            let Some(index) = entry_index(track, payload.region_id) else {

                return false;

            };

            track.entries.remove(index);

            publish(track);

            true

        }

        HIRARI_REGION_EDIT_RESTORE => {

            let Some(snapshot) = (unsafe { (payload.blob as *const RegionSnapshot).as_ref() })

            else {

                return false;

            };

            // Undo replays a whole-region snapshot over whatever the region has

            // become, so a live entry is dropped first; its retained audio

            // handle is released by that drop and the snapshot keeps its own.

            if let Some(index) = entry_index(track, payload.region_id) {

                track.entries.remove(index);

            }

            if !add_snapshot(track, payload.region_id, snapshot) {

                return false;

            }

            publish(track);

            true

        }

        HIRARI_REGION_EDIT_NOTE_STORE => {

            let Some(index) = entry_index(track, payload.region_id) else {

                return false;

            };

            let store = ptr::from_mut(&mut track.entries[index].notes);

            if !unsafe { hirari_region_note_store_restore(payload.blob as *mut c_void, store) } {

                return false;

            }

            publish(track);

            true

        }

        HIRARI_REGION_EDIT_GEOMETRY => {

            let Some(markers) = (unsafe { blob_span::<HirariWarpMarker>(payload) }) else {

                return false;

            };

            let Some(index) = entry_index(track, payload.region_id) else {

                return false;

            };

            let locked = track.entries[index].state.locked;

            if locked {

                return false;

            }

            let clip = ptr::from_mut(&mut track.entries[index].clip);

            if unsafe {

                hirari_region_clip_edits_set_warp_markers(clip, markers.as_ptr(), markers.len(), 0, 0, false)

            } < 0

            {

                return false;

            }

            let state = payload.state;

            let entry = &mut track.entries[index];

            entry.state.start = state.start;

            entry.state.len = state.len;

            entry.state.source_offset = state.source_offset;

            entry.state.source_length = state.source_length;

            entry.state.fade_in_samples = state.fade_in_samples;

            entry.state.fade_out_samples = state.fade_out_samples;

            entry.state.warp_ratio = state.warp_ratio;

            publish(track);

            true

        }

        HIRARI_REGION_EDIT_MARKERS => {

            let Some(markers) = (unsafe { blob_span::<HirariWarpMarker>(payload) }) else {

                return false;

            };

            let Some(index) = entry_index(track, payload.region_id) else {

                return false;

            };

            let entry = &mut track.entries[index];

            if entry.state.locked {

                return false;

            }

            let source_span = if entry.state.source_length > 0 {

                entry.state.source_length

            } else {

                entry.state.len

            };

            let timeline = entry.state.len;

            let clip = ptr::from_mut(&mut entry.clip);

            let applied = unsafe {

                hirari_region_clip_edits_set_warp_markers(

                    clip,

                    markers.as_ptr(),

                    markers.len(),

                    source_span,

                    timeline,

                    true,

                )

            };

            if applied < 0 {

                return false;

            }

            if applied > 0 {

                publish(track);

            }

            true

        }

        HIRARI_REGION_EDIT_RANGE_EDITS => {

            let Some(edits) = (unsafe { blob_span::<NativeRangeEdit>(payload) }) else {

                return false;

            };

            let Some(index) = entry_index(track, payload.region_id) else {

                return false;

            };

            if track.entries[index].state.locked {

                return false;

            }

            let length = track.entries[index].state.len;

            let clip = ptr::from_mut(&mut track.entries[index].clip);

            if unsafe {

                hirari_region_clip_edits_replace_range_edits(clip, edits.as_ptr(), edits.len(), length, false)

            } {

                publish(track);

                return true;

            }

            false

        }

        HIRARI_REGION_EDIT_COMP_RANGES => {

            let Some(ranges) = (unsafe { blob_span::<NativeCompRange>(payload) }) else {

                return false;

            };

            let Some(index) = entry_index(track, payload.region_id) else {

                return false;

            };

            let managed = payload.managed;

            let clip = ptr::from_mut(&mut track.entries[index].clip);

            if unsafe {

                hirari_region_clip_edits_replace_comp_ranges(clip, ranges.as_ptr(), ranges.len(), managed)

            } {

                publish(track);

                return true;

            }

            false

        }

        HIRARI_REGION_EDIT_PITCH_COMMIT => {

            let analysis = payload.blob as *mut c_void;

            if analysis.is_null() {

                return false;

            }

            let current_generation = track.publications;

            let index = entry_index(track, payload.region_id);

            let accepted = index.is_some()

                && (current_generation == payload.generation || payload.generation == 0);

            let mut applied = false;

            if accepted {

                let store = ptr::from_mut(&mut track.entries[index.unwrap()].notes).cast();

                applied = unsafe { hirari_audio_pitch_commit_region_result(analysis, store) };

                if applied {

                    publish(track);

                }

            }

            unsafe { hirari_audio_pitch_result_destroy(analysis) };

            applied

        }

        HIRARI_REGION_EDIT_REPLACE_AUDIO => {

            if payload.blob.is_null() || payload.blob_len < HANDLE_BYTES {

                return false;

            }

            let mut raw = [0u8; HANDLE_BYTES];

            raw.copy_from_slice(unsafe { slice::from_raw_parts(payload.blob, HANDLE_BYTES) });

            let handle = usize::from_ne_bytes(raw) as *mut c_void;

            let path = bytes_of(unsafe { payload.blob.add(HANDLE_BYTES) }, payload.blob_len - HANDLE_BYTES).to_vec();

            let Some(index) = entry_index(track, payload.region_id) else {

                release_audio(handle);

                return false;

            };

            let entry = &mut track.entries[index];

            let previous = unsafe { ptr::replace(&mut entry.audio, handle) };

            release_audio(previous);

            entry.state = payload.state;

            entry.path = path.clone();

            entry.name = path;

            // The previous source's warp markers described a different file.

            let clip = ptr::from_mut(&mut entry.clip);

            unsafe {

                hirari_region_clip_edits_set_warp_markers(clip, ptr::null(), 0, 0, 0, false);

            }

            publish(track);

            true

        }

        _ => {

            let Some(index) = entry_index(track, payload.region_id) else {

                return false;

            };

            let entry = &mut track.entries[index];

            let state = payload.state;

            if state.pitch_preserve_warp != entry.state.pitch_preserve_warp

                && entry.state.source_length == 0

            {

                // The renderer needs the untrimmed length to build the warp map.

                entry.state.source_length = entry.state.len;

            }

            entry.state.clip_gain = state.clip_gain;

            entry.state.muted = state.muted;

            entry.state.reverse = state.reverse;

            entry.state.pitch_semitones = state.pitch_semitones;

            entry.state.loop_count = state.loop_count;

            entry.state.sync_group = state.sync_group;

            entry.state.fade_in_samples = state.fade_in_samples;

            entry.state.fade_out_samples = state.fade_out_samples;

            entry.state.locked = state.locked;

            entry.state.pitch_preserve_warp = state.pitch_preserve_warp;

            publish(track);

            true

        }

    }

}



/// Borrows a payload blob as a slice of `T`, rejecting a ragged length or a

/// missing buffer for a non-zero count.

///

/// # Safety

/// `payload.blob` is readable for `payload.blob_len` bytes.

unsafe fn blob_span<T>(payload: &HirariRegionEditPayload) -> Option<&[T]> {

    let size = std::mem::size_of::<T>();

    if size == 0 || payload.blob_len % size != 0 {

        return None;

    }

    let count = payload.blob_len / size;

    if count == 0 {

        return Some(&[]);

    }

    if payload.blob.is_null() {

        return None;

    }

    Some(unsafe { slice::from_raw_parts(payload.blob.cast::<T>(), count) })

}



/// Clears a region's processing history. Returns false when the region is gone.

#[no_mangle]

pub extern "C" fn hirari_region_store_clear_processing(key: *mut c_void, region_id: u32) -> bool {

    let Ok(mut tracks) = store().lock() else {

        return false;

    };

    let Some(track) = tracks.iter_mut().find(|stored| stored.host == key) else {

        return false;

    };

    let Some(index) = entry_index(track, region_id) else {

        return false;

    };

    if unsafe { hirari_region_processing_history_count(track.entries[index].history) } == 0 {

        return false;

    }

    unsafe { hirari_region_processing_history_reset(track.entries[index].history) };

    invalidate_frozen(track.host);

    publish(track);

    true

}



/// Appends one processing step. The store owns the admissibility policy.

///

/// # Safety

/// `step.operation` is readable for its length.

#[no_mangle]

pub unsafe extern "C" fn hirari_region_store_append_processing(

    key: *mut c_void,

    region_id: u32,

    step: HirariEventStep,

) -> bool {

    let Ok(mut tracks) = store().lock() else {

        return false;

    };

    let Some(track) = tracks.iter_mut().find(|stored| stored.host == key) else {

        return false;

    };

    let Some(index) = entry_index(track, region_id) else {

        return false;

    };

    if unsafe { !hirari_region_processing_history_append(track.entries[index].history, step) } {

        return false;

    }

    invalidate_frozen(track.host);

    publish(track);

    true

}



#[cfg(test)]

#[path = "region_store_tests.rs"]

mod region_store_tests;


