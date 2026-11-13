//! Host-driven region edits.
//!
//! Region storage stays in the C++ engine because it owns the audio buffers
//! and the immutable snapshot the audio callback reads. This module owns the
//! edit policy and the undo capture: it reads the current state through a host
//! callback, decides, asks the host to apply, and records exactly one undo
//! entry. The C++ side keeps no per-field edit code.
//!
//! Every callback here runs on the control thread. Nothing in this module is
//! reachable from the audio callback.

#[path = "region_replay_recording.rs"]
mod replay_recording;
pub(crate) use replay_recording::*;

use std::ffi::c_char;

use crate::audio_note_curve::{
    hirari_audio_note_curve_component_anchor, hirari_audio_note_curve_move_anchor,
    HirariAudioNoteAnchor,
};
use crate::vari_audio::{
    hirari_region_vari_audio_store_clear_segments, hirari_region_vari_audio_store_new,
    hirari_region_vari_audio_store_remove, hirari_region_vari_audio_store_split,
    hirari_region_vari_audio_store_upsert, hirari_region_vari_audio_store_upsert_anchor,
    hirari_region_vari_audio_store_warp, vari_store_clone, vari_store_release, vari_store_restore,
    vari_store_view, HirariAudioNoteSegment, HirariRegionVariAudioStore,
};
use std::os::raw::c_void;
use std::ptr;
use std::slice;
use std::sync::{Mutex, OnceLock};

use crate::region_comp_ranges::{hirari_region_comp_ranges_validate, NativeCompRange};
use crate::region_mutations::hirari_region_sync_move_positions;
use crate::region_processing::{
    hirari_free_range_edits, hirari_region_fade_samples, hirari_region_range_edits_apply,
    hirari_region_set_trim_geometry, hirari_region_set_warp_ratio_geometry, NativeRangeEdit,
    HirariRegionScalarDecision, HirariRegionScalarEdit, HirariRegionScalarField,
    NativeRegionGeometry,
};
use crate::region_split::{
    hirari_region_duplicate_state, hirari_region_split_plan, hirari_region_split_plan_free,
    HirariRegionSplitInput, HirariRegionSplitOutput,
};
use crate::region_warp::{
    hirari_free_warp_markers, hirari_region_normalize_warp_markers, HirariWarpMarker,
};
use crate::undo_transaction::{
    hirari_undo_manager_commit, hirari_undo_manager_discard_native_callback,
    hirari_undo_manager_register_native_callback,
};

/// Payload op: write the policy-controlled scalar fields in place.
pub const HIRARI_REGION_EDIT_INPLACE: i32 = 2;
/// Payload op: replace the region's editable state and republish it.
pub const HIRARI_REGION_EDIT_STATE: i32 = 1;
/// Payload op: write the geometry fields Rust computed for a trim or warp
/// ratio edit and replace the warp markers from `blob`.
pub const HIRARI_REGION_EDIT_GEOMETRY: i32 = 14;
/// Payload op: remove the region and drop its cached peaks.
pub const HIRARI_REGION_EDIT_REMOVE: i32 = 12;
/// Payload op: put a removed region back from a host-held snapshot handle. The
/// host takes ownership of the handle when the payload is recorded for undo.
pub const HIRARI_REGION_EDIT_RESTORE: i32 = 13;
/// Payload op: replace only the warp markers from `blob`, leaving the editable
/// state alone. Timing jobs (alignment, quantize) commit through this.
pub const HIRARI_REGION_EDIT_MARKERS: i32 = 15;
/// Payload op: store the range-edit list packed in `blob` exactly as given.
pub const HIRARI_REGION_EDIT_RANGE_EDITS: i32 = 16;
/// Payload op: store the comp-range batch packed in `blob` plus `managed`
/// exactly as given.
pub const HIRARI_REGION_EDIT_COMP_RANGES: i32 = 17;
/// Payload op: commit an owned F0 analysis result (blob) into the region's
/// note table when the region still matches `state` at `generation`. The host
/// takes the analysis handle over on every path.
pub const HIRARI_REGION_EDIT_PITCH_COMMIT: i32 = 19;
/// Payload op: swap in a decoded audio handle. The blob starts with the owned
/// handle followed by the path bytes; the resolved rates and timeline geometry
/// travel in the payload state.
pub const HIRARI_REGION_EDIT_REPLACE_AUDIO: i32 = 18;
const REPLACE_AUDIO_HEADER_BYTES: usize = 8;
const _: () = assert!(std::mem::size_of::<usize>() == 8);

/// The editable part of a region. Mirrors the C++ `HirariRegionState` byte for
/// byte; audio buffers, note edits and processing history stay in C++.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariRegionState {
    pub start: u64,
    pub len: u64,
    pub base_start: u64,
    pub base_length: u64,
    pub source_offset: u64,
    pub base_source_offset: u64,
    pub source_length: u64,
    pub fade_in_samples: u64,
    pub fade_out_samples: u64,
    pub warp_ratio: f64,
    pub source_sample_rate: f64,
    pub timeline_sample_rate: f64,
    pub clip_gain: f32,
    pub pitch_semitones: f32,
    pub loop_count: u32,
    pub sync_group: u32,
    pub muted: bool,
    pub reverse: bool,
    pub locked: bool,
    pub pitch_preserve_warp: bool,
}

// Bitwise float comparison: a region whose rate is NaN is still the same
// region, and the tests compare states that crossed the C++ ABI.
impl PartialEq for HirariRegionState {
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start
            && self.len == other.len
            && self.base_start == other.base_start
            && self.base_length == other.base_length
            && self.source_offset == other.source_offset
            && self.base_source_offset == other.base_source_offset
            && self.source_length == other.source_length
            && self.fade_in_samples == other.fade_in_samples
            && self.fade_out_samples == other.fade_out_samples
            && self.warp_ratio.to_bits() == other.warp_ratio.to_bits()
            && self.source_sample_rate.to_bits() == other.source_sample_rate.to_bits()
            && self.timeline_sample_rate.to_bits() == other.timeline_sample_rate.to_bits()
            && self.clip_gain.to_bits() == other.clip_gain.to_bits()
            && self.pitch_semitones.to_bits() == other.pitch_semitones.to_bits()
            && self.loop_count == other.loop_count
            && self.sync_group == other.sync_group
            && self.muted == other.muted
            && self.reverse == other.reverse
            && self.locked == other.locked
            && self.pitch_preserve_warp == other.pitch_preserve_warp
    }
}

/// Source frames consumed per project-timeline frame. The replace-audio
/// geometry, the split/trim mapping and the spectral-stretch decision all read
/// this one function, including the fallbacks for unset rates.
pub(crate) fn source_frames_per_timeline_frame(
    warp_ratio: f64,
    source_sample_rate: f64,
    timeline_sample_rate: f64,
) -> f64 {
    let source_rate = if source_sample_rate.is_finite() && source_sample_rate > 0.0 {
        source_sample_rate
    } else {
        timeline_sample_rate
    };
    let timeline_rate = if timeline_sample_rate.is_finite() && timeline_sample_rate > 0.0 {
        timeline_sample_rate
    } else {
        source_rate
    };
    let conversion = source_rate / timeline_rate;
    let effective = warp_ratio * conversion;
    if effective.is_finite() && effective > 0.0 {
        effective
    } else {
        warp_ratio
    }
}

impl HirariRegionState {
    pub(crate) fn source_frames_per_timeline_frame(&self) -> f64 {
        source_frames_per_timeline_frame(
            self.warp_ratio,
            self.source_sample_rate,
            self.timeline_sample_rate,
        )
    }
}

/// One reversible edit. The C++ host applies `HIRARI_REGION_EDIT_SCALAR` through
/// the track's scalar path and `HIRARI_REGION_EDIT_STATE` by replacing the
/// editable state and republishing (the same normalisation `addRegion` runs).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariRegionEditPayload {
    pub track_id: u32,
    pub region_id: u32,
    pub op: i32,
    /// Packed payload, or an owned note-table snapshot handle for
    /// `HIRARI_REGION_EDIT_NOTE_STORE`. The host takes ownership in that case.
    pub blob: *const u8,
    pub blob_len: usize,
    pub state: HirariRegionState,
    /// Comp-range batch flag; unused by the other ops.
    pub managed: bool,
    /// Region-table generation the payload was decided against; 0 skips the
    /// staleness check. Only the pitch commit reads it.
    pub generation: u64,
}

// The payload only borrows a buffer (or hands over an opaque snapshot handle)
// for the duration of one host call; Rust never dereferences it.
unsafe impl Send for HirariRegionEditPayload {}
unsafe impl Sync for HirariRegionEditPayload {}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariRegionGroupMember {
    pub track_id: u32,
    pub region_id: u32,
    pub start: u64,
    pub base_start: u64,
    pub locked: bool,
}

pub type HirariRegionFindFn =
    unsafe extern "C" fn(*mut c_void, u32, u32, *mut HirariRegionState) -> bool;
pub type HirariRegionApplyFn =
    unsafe extern "C" fn(*mut c_void, *const HirariRegionEditPayload) -> bool;
pub type HirariRegionListGroupFn =
    unsafe extern "C" fn(*mut c_void, u32, *mut HirariRegionGroupMember, usize) -> usize;
/// Copies a region's warp markers into `out` and returns the total count. A
/// null or short buffer reports the count without writing, like the sync-group
/// listing.
pub type HirariRegionMarkersFn =
    unsafe extern "C" fn(*mut c_void, u32, u32, *mut HirariWarpMarker, usize) -> usize;
/// Copies a region's range edits into `out` and returns the total count, with
/// the same count-then-copy contract as the warp-marker listing.
pub type HirariRegionRangeEditsFn =
    unsafe extern "C" fn(*mut c_void, u32, u32, *mut NativeRangeEdit, usize) -> usize;
pub type HirariRegionNoteEditFn =
    unsafe extern "C" fn(*mut c_void, *mut HirariRegionVariAudioStore) -> bool;
pub type HirariRegionNoteEditHostFn = unsafe extern "C" fn(
    *mut c_void,
    u32,
    u32,
    *mut c_void,
    HirariRegionNoteEditFn,
) -> bool;
pub type HirariRegionRecordUndoFn = unsafe extern "C" fn(
    *mut c_void,
    *const c_char,
    *const HirariRegionEditPayload,
    usize,
    *const HirariRegionEditPayload,
    usize,
    bool,
    bool,
) -> bool;
/// Copies the whole region (audio, history, note edits) into an owned host
/// handle, or returns null when the region does not exist.
pub type HirariRegionSnapshotFn = unsafe extern "C" fn(*mut c_void, u32, u32) -> *mut c_void;
/// Frees a handle from `HirariRegionSnapshotFn` that was never recorded.
pub type HirariRegionReleaseSnapshotFn = unsafe extern "C" fn(*mut c_void, *mut c_void);
/// Retains a region's decoded audio in an immutable host handle so a worker
/// thread can read it after the control thread moves on. Null when the region
/// has no decoded audio.
pub type HirariRegionRetainAudioFn = unsafe extern "C" fn(*mut c_void, u32, u32) -> *mut c_void;
/// Reports a retained handle's channel count. With room in `out`, also writes
/// the channel pointers and the total sample count.
pub type HirariRegionAudioViewFn =
    unsafe extern "C" fn(*mut c_void, *mut *const f32, usize, *mut u64) -> u32;
/// Releases a handle from `HirariRegionRetainAudioFn`.
pub type HirariRegionReleaseAudioFn = unsafe extern "C" fn(*mut c_void);
/// Reports a track's region-table generation and published snapshot identity.
/// Returns false when the track is no longer part of the project.
pub type HirariRegionTrackTokenFn =
    unsafe extern "C" fn(*mut c_void, u32, *mut u64, *mut usize) -> bool;
/// Lists track ids in engine order. Reports the total even when `out` is short.
pub type HirariRegionListTracksFn = unsafe extern "C" fn(*mut c_void, *mut u32, usize) -> usize;
/// Lists one track's region ids in storage order.
pub type HirariRegionListRegionsFn = unsafe extern "C" fn(*mut c_void, u32, *mut u32, usize) -> usize;
/// Returns the Rust-owned tempo map handle.
pub type HirariRegionTempoStateFn = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
/// Returns the Rust-owned undo history handle.
pub type HirariRegionUndoStateFn = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
/// Returns the project timeline sample rate.
pub type HirariRegionSampleRateFn = unsafe extern "C" fn(*mut c_void) -> f64;
/// Copies a region's source path into `out` and returns its byte length. A
/// null or short buffer reports the length without writing.
pub type HirariRegionPathFn =
    unsafe extern "C" fn(*mut c_void, u32, u32, *mut u8, usize) -> usize;
/// Inserts a freshly decoded region under the id the caller allocated. The
/// audio handle is consumed on every path. Returns the new region id, or 0 on
/// failure.
pub type HirariRegionInsertFn = unsafe extern "C" fn(
    *mut c_void,
    u32,
    usize,
    *const u8,
    usize,
    u64,
    f64,
    f64,
    u64,
    u32,
    *mut c_void,
) -> u32;
/// Reports the live track's identity, or 0 when the id is not in the project.
pub type HirariRegionTrackIdentityFn =
    unsafe extern "C" fn(*mut c_void, u32) -> usize;
/// Retains a track in an owned handle so a worker can use it safely.
pub type HirariRegionRetainTrackFn = unsafe extern "C" fn(*mut c_void, u32) -> *mut c_void;
/// Releases a handle from `HirariRegionRetainTrackFn`.
pub type HirariRegionReleaseTrackFn = unsafe extern "C" fn(*mut c_void);
/// Starts F0 analysis ownership on a retained track.
pub type HirariRegionPitchBeginFn = unsafe extern "C" fn(*mut c_void) -> bool;
/// Ends F0 analysis ownership on a retained track.
pub type HirariRegionPitchFinishFn = unsafe extern "C" fn(*mut c_void);
/// Copies a region's note table from a retained track into an owned store.
pub type HirariRegionNoteSnapshotFn =
    unsafe extern "C" fn(*mut c_void, u32) -> *mut c_void;
/// Clones a region under the id the caller allocated, optionally replacing its
/// editable state first, and adds the copy. Returns the new id, or 0 on failure.
pub type HirariRegionCloneFn = unsafe extern "C" fn(
    *mut c_void,
    u32,
    u32,
    *const HirariRegionState,
    u32,
) -> u32;
/// Copies a region's comp ranges into `out` and reports the managed flag,
/// with the same count-then-copy contract as the range-edit listing.
pub type HirariRegionCompRangesFn =
    unsafe extern "C" fn(*mut c_void, u32, u32, *mut NativeCompRange, usize, *mut bool) -> usize;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariRegionHostOps {
    pub find_region: Option<HirariRegionFindFn>,
    pub apply_edit: Option<HirariRegionApplyFn>,
    pub list_sync_group: Option<HirariRegionListGroupFn>,
    pub record_undo: Option<HirariRegionRecordUndoFn>,
    pub note_edit: Option<HirariRegionNoteEditHostFn>,
    pub snapshot_region: Option<HirariRegionSnapshotFn>,
    pub release_snapshot: Option<HirariRegionReleaseSnapshotFn>,
    pub warp_markers: Option<HirariRegionMarkersFn>,
    pub range_edits: Option<HirariRegionRangeEditsFn>,
    pub retain_audio: Option<HirariRegionRetainAudioFn>,
    pub audio_view: Option<HirariRegionAudioViewFn>,
    pub release_audio: Option<HirariRegionReleaseAudioFn>,
    pub track_token: Option<HirariRegionTrackTokenFn>,
    pub list_tracks: Option<HirariRegionListTracksFn>,
    pub list_regions: Option<HirariRegionListRegionsFn>,
    pub tempo_state: Option<HirariRegionTempoStateFn>,
    pub undo_state: Option<HirariRegionUndoStateFn>,
    pub sample_rate: Option<HirariRegionSampleRateFn>,
    pub region_path: Option<HirariRegionPathFn>,
    pub insert_region: Option<HirariRegionInsertFn>,
    pub track_identity: Option<HirariRegionTrackIdentityFn>,
    pub retain_track: Option<HirariRegionRetainTrackFn>,
    pub release_track: Option<HirariRegionReleaseTrackFn>,
    pub pitch_begin: Option<HirariRegionPitchBeginFn>,
    pub pitch_finish: Option<HirariRegionPitchFinishFn>,
    pub note_snapshot: Option<HirariRegionNoteSnapshotFn>,
    pub clone_region: Option<HirariRegionCloneFn>,
    pub comp_ranges: Option<HirariRegionCompRangesFn>,
}

// The table only holds function pointers supplied by the native engine; it
// carries no data the Rust side mutates.
unsafe impl Send for HirariRegionHostOps {}
unsafe impl Sync for HirariRegionHostOps {}

static HOST_OPS: OnceLock<Mutex<Option<HirariRegionHostOps>>> = OnceLock::new();

pub(crate) fn host_ops() -> Option<HirariRegionHostOps> {
    HOST_OPS.get()?.lock().ok().and_then(|guard| *guard)
}

/// Store lookups are keyed by the live `Track` the host holds, because track
/// ids only name a track inside one engine.
pub(crate) fn store_key(host: *mut c_void, track_id: u32) -> *mut c_void {
    let Some(ops) = host_ops() else {
        return ptr::null_mut();
    };
    let Some(identity) = ops.track_identity else {
        return ptr::null_mut();
    };
    match unsafe { identity(host, track_id) } {
        0 => ptr::null_mut(),
        value => value as *mut c_void,
    }
}

extern "C" fn store_find(host: *mut c_void, track_id: u32, region_id: u32,
                         out: *mut HirariRegionState) -> bool {
    let key = store_key(host, track_id);
    unsafe { crate::region_store::hirari_region_store_find(key, region_id, out) }
}

extern "C" fn store_apply_edit(host: *mut c_void, payload: *const HirariRegionEditPayload) -> bool {
    let track_id = unsafe { (*payload).track_id };
    let key = store_key(host, track_id);
    unsafe { crate::region_store::hirari_region_store_apply_edit(host, key, payload) }
}

extern "C" fn store_list_sync_group(host: *mut c_void, group: u32,
                                    out: *mut HirariRegionGroupMember, capacity: usize) -> usize {
    unsafe { crate::region_store::hirari_region_store_list_sync_group(host, group, out, capacity) }
}

extern "C" fn store_note_edit(host: *mut c_void, track_id: u32, region_id: u32,
                              context: *mut c_void, mutate: HirariRegionNoteEditFn) -> bool {
    let key = store_key(host, track_id);
    unsafe { crate::region_store::hirari_region_store_note_edit(key, region_id, context, mutate) }
}

extern "C" fn store_snapshot_region(host: *mut c_void, track_id: u32, region_id: u32) -> *mut c_void {
    let key = store_key(host, track_id);
    crate::region_store::hirari_region_store_snapshot_region(key, region_id)
}

extern "C" fn store_release_snapshot(_host: *mut c_void, handle: *mut c_void) {
    unsafe { crate::region_store::hirari_region_store_release_snapshot(handle) };
}

extern "C" fn store_warp_markers(host: *mut c_void, track_id: u32, region_id: u32,
                                 out: *mut crate::region_warp::HirariWarpMarker,
                                 capacity: usize) -> usize {
    let key = store_key(host, track_id);
    unsafe { crate::region_store::hirari_region_store_warp_markers(key, region_id, out, capacity) }
}

extern "C" fn store_range_edits(host: *mut c_void, track_id: u32, region_id: u32,
                                out: *mut crate::region_processing::NativeRangeEdit,
                                capacity: usize) -> usize {
    let key = store_key(host, track_id);
    unsafe { crate::region_store::hirari_region_store_range_edits(key, region_id, out, capacity) }
}

extern "C" fn store_comp_ranges(host: *mut c_void, track_id: u32, region_id: u32,
                                out: *mut crate::region_comp_ranges::NativeCompRange,
                                capacity: usize, managed: *mut bool) -> usize {
    let key = store_key(host, track_id);
    unsafe { crate::region_store::hirari_region_store_comp_ranges(key, region_id, out, capacity, managed) }
}

extern "C" fn store_retain_audio(host: *mut c_void, track_id: u32, region_id: u32) -> *mut c_void {
    let key = store_key(host, track_id);
    crate::region_store::hirari_region_store_retain_audio(key, region_id)
}

extern "C" fn store_track_token(host: *mut c_void, track_id: u32, generation: *mut u64,
                                snapshot: *mut usize) -> bool {
    let key = store_key(host, track_id);
    unsafe { crate::region_store::hirari_region_store_track_token(key, generation, snapshot) }
}

/// Lists a track's region ids for the host tables that address tracks by id.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_store_list_regions(host: *mut c_void, track_id: u32,
                                                          out: *mut u32,
                                                          capacity: usize) -> usize {
    let key = store_key(host, track_id);
    crate::region_store::hirari_region_store_list_track_regions(key, out, capacity)
}

extern "C" fn store_region_path(host: *mut c_void, track_id: u32, region_id: u32, out: *mut u8,
                                capacity: usize) -> usize {
    let key = store_key(host, track_id);
    unsafe { crate::region_store::hirari_region_store_region_path(key, region_id, out, capacity) }
}

extern "C" fn store_insert_region(host: *mut c_void, _track_id: u32, expected: usize,
                                  path: *const u8, length: usize, start: u64, source_rate: f64,
                                  timeline_rate: f64, duration: u64, region_id: u32,
                                  audio: *mut c_void) -> u32 {
    let _ = host;
    unsafe {
        crate::region_store::hirari_region_store_insert_region(
            expected as *mut c_void, path, length, start, source_rate, timeline_rate,
            duration, region_id, audio
        )
    }
}

extern "C" fn store_clone_region(host: *mut c_void, track_id: u32, region_id: u32,
                                 state: *const HirariRegionState, new_id: u32) -> u32 {
    let key = store_key(host, track_id);
    unsafe { crate::region_store::hirari_region_store_clone_region(key, region_id, state, new_id) }
}

/// Installs the native host callbacks. Passing null clears them.
///
/// The region-data callbacks are answered by the Rust region store
/// (`region_store.rs`); the host table only supplies the callbacks that need
/// the native engine — decoded-buffer access, track listing, tempo, undo state
/// and media decoding.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_host_install(ops: *const HirariRegionHostOps) {
    let slot = HOST_OPS.get_or_init(|| Mutex::new(None));
    let value = if ops.is_null() {
        None
    } else {
        let mut ops = unsafe { ptr::read(ops) };
        if crate::region_store::region_data_installed() {
            ops.find_region = Some(store_find);
            ops.apply_edit = Some(store_apply_edit);
            ops.list_sync_group = Some(store_list_sync_group);
            ops.note_edit = Some(store_note_edit);
            ops.snapshot_region = Some(store_snapshot_region);
            ops.release_snapshot = Some(store_release_snapshot);
            ops.warp_markers = Some(store_warp_markers);
            ops.range_edits = Some(store_range_edits);
            ops.comp_ranges = Some(store_comp_ranges);
            ops.retain_audio = Some(store_retain_audio);
            ops.track_token = Some(store_track_token);
            ops.list_regions = Some(hirari_region_store_list_regions);
            ops.region_path = Some(store_region_path);
            ops.insert_region = Some(store_insert_region);
            ops.clone_region = Some(store_clone_region);
        }
        Some(ops)
    };
    if let Ok(mut guard) = slot.lock() {
        *guard = value;
    }
}

fn scalar_field(value: i32) -> Option<HirariRegionScalarField> {
    Some(match value {
        1 => HirariRegionScalarField::ClipGain,
        2 => HirariRegionScalarField::Muted,
        3 => HirariRegionScalarField::Reverse,
        4 => HirariRegionScalarField::PitchSemitones,
        5 => HirariRegionScalarField::LoopCount,
        6 => HirariRegionScalarField::SyncGroup,
        7 => HirariRegionScalarField::Fades,
        8 => HirariRegionScalarField::Locked,
        9 => HirariRegionScalarField::PitchPreserveWarp,
        _ => return None,
    })
}

/// Undo labels, kept identical to the strings the C++ layer used to record.
/// The undo label, or `None` for fields the engine edited without one.
fn field_name(field: HirariRegionScalarField) -> Option<&'static [u8]> {
    Some(match field {
        HirariRegionScalarField::ClipGain => b"Region Gain\0",
        HirariRegionScalarField::Muted => b"Mute Region\0",
        HirariRegionScalarField::Reverse => b"Reverse Region\0",
        HirariRegionScalarField::PitchSemitones => b"Flex Pitch\0",
        HirariRegionScalarField::LoopCount => b"Region Loop\0",
        HirariRegionScalarField::SyncGroup => b"Region Sync Group\0",
        HirariRegionScalarField::Fades => b"Region Fades\0",
        HirariRegionScalarField::Locked => return None,
        HirariRegionScalarField::PitchPreserveWarp => b"Preserve Warp Pitch\0",
    })
}

fn state_payload(
    track_id: u32,
    region_id: u32,
    op: i32,
    state: HirariRegionState,
) -> HirariRegionEditPayload {
    HirariRegionEditPayload {
        track_id,
        region_id,
        op,
        state,
        ..Default::default()
    }
}

/// Upper bound for a captured marker list. The engine rejects edits above this
/// order of magnitude; the cap only keeps a foreign host from driving an
/// unbounded allocation here.
const MAX_WARP_MARKERS: usize = 1_000_000;

fn geometry_of(state: HirariRegionState) -> NativeRegionGeometry {
    NativeRegionGeometry {
        start: state.start,
        length: state.len,
        source_length: state.source_length,
        source_offset: state.source_offset,
        base_start: state.base_start,
        base_source_offset: state.base_source_offset,
        base_length: state.base_length,
        loop_count: state.loop_count,
        fade_in: state.fade_in_samples,
        fade_out: state.fade_out_samples,
        warp_ratio: state.warp_ratio,
        source_sample_rate: state.source_sample_rate,
        timeline_sample_rate: state.timeline_sample_rate,
    }
}

fn state_with_geometry(
    state: HirariRegionState,
    geometry: &NativeRegionGeometry,
) -> HirariRegionState {
    let mut after = state;
    after.start = geometry.start;
    after.len = geometry.length;
    after.source_offset = geometry.source_offset;
    after.source_length = geometry.source_length;
    after.fade_in_samples = geometry.fade_in;
    after.fade_out_samples = geometry.fade_out;
    after.warp_ratio = geometry.warp_ratio;
    after
}

pub(crate) fn warp_markers_of(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> Option<Vec<HirariWarpMarker>> {
    let markers: HirariRegionMarkersFn = ops.warp_markers?;
    let count = unsafe { markers(host, track_id, region_id, ptr::null_mut(), 0) };
    if count > MAX_WARP_MARKERS {
        return None;
    }
    let mut captured = vec![HirariWarpMarker::default(); count];
    if count == 0 {
        return Some(captured);
    }
    let written = unsafe { markers(host, track_id, region_id, captured.as_mut_ptr(), count) };
    if written != count {
        return None;
    }
    Some(captured)
}

/// Runs the shared marker normalizer and adopts its output. An empty list is
/// valid and clears the markers; a null result means normalization failed.
fn normalize_markers(
    markers: &[HirariWarpMarker],
    source_span: u64,
    timeline_length: u64,
) -> Option<Vec<HirariWarpMarker>> {
    if markers.is_empty() {
        return Some(Vec::new());
    }
    let mut count = 0usize;
    let out = unsafe {
        hirari_region_normalize_warp_markers(
            markers.as_ptr(),
            markers.len(),
            source_span,
            timeline_length,
            &mut count,
        )
    };
    if out.is_null() {
        return None;
    }
    let ordered = unsafe { std::slice::from_raw_parts(out, count) }.to_vec();
    unsafe { hirari_free_warp_markers(out, count) };
    Some(ordered)
}

fn same_markers(left: &[HirariWarpMarker], right: &[HirariWarpMarker]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(a, b)| {
            a.source_sample == b.source_sample
                && a.timeline_sample == b.timeline_sample
                && a.transient == b.transient
        })
}

/// The previous engine setter rejected edits above this order of magnitude.
const MAX_WARP_MARKER_EDITS: usize = 100_000;

/// Replaces a region's warp markers. The previous engine setter rejected any
/// list that normalization would shorten, so callers can rely on a marker
/// count round-trip; a successful edit records one marker-aware undo entry.
fn set_warp_markers_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    markers: *const HirariWarpMarker,
    count: usize,
) -> bool {
    if track_id == 0 || region_id == 0 || count > MAX_WARP_MARKER_EDITS {
        return false;
    }
    if count != 0 && markers.is_null() {
        return false;
    }
    let (Some(find), Some(apply)) = (ops.find_region, ops.apply_edit) else {
        return false;
    };
    let mut state = HirariRegionState::default();
    if !unsafe { find(host, track_id, region_id, &mut state) } || state.locked {
        return false;
    }
    let input: &[HirariWarpMarker] = if count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(markers, count) }
    };
    let Some(ordered) = normalize_markers(input, u64::MAX, u64::MAX) else {
        return false;
    };
    if ordered.len() != input.len() {
        return false;
    }
    let source_span = if state.source_length > 0 {
        state.source_length
    } else {
        state.len
    };
    let Some(ordered) = normalize_markers(&ordered, source_span, state.len) else {
        return false;
    };
    if ordered.len() != input.len() {
        return false;
    }
    let Some(current) = warp_markers_of(ops, host, track_id, region_id) else {
        return false;
    };
    if same_markers(&current, &ordered) {
        return true;
    }
    let undo = geometry_payload(track_id, region_id, state, &current);
    let redo = geometry_payload(track_id, region_id, state, &ordered);
    if !unsafe { apply(host, &redo) } {
        return false;
    }
    record(
        ops,
        host,
        b"Edit Region Warp Markers\0",
        &[undo],
        &[redo],
        false,
    );
    true
}

fn geometry_payload(
    track_id: u32,
    region_id: u32,
    state: HirariRegionState,
    markers: &[HirariWarpMarker],
) -> HirariRegionEditPayload {
    HirariRegionEditPayload {
        track_id,
        region_id,
        op: HIRARI_REGION_EDIT_GEOMETRY,
        blob: if markers.is_empty() {
            ptr::null()
        } else {
            markers.as_ptr().cast()
        },
        blob_len: std::mem::size_of_val(markers),
        state,
        ..Default::default()
    }
}

/// Applies a validated geometry result and records one marker-aware undo entry.
fn commit_geometry(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    before: HirariRegionState,
    geometry: NativeRegionGeometry,
    name: &[u8],
) -> bool {
    let Some(apply) = ops.apply_edit else {
        return false;
    };
    let Some(markers) = warp_markers_of(ops, host, track_id, region_id) else {
        return false;
    };
    let after = state_with_geometry(before, &geometry);
    let undo = geometry_payload(track_id, region_id, before, &markers);
    let redo = geometry_payload(track_id, region_id, after, &[]);
    if !unsafe { apply(host, &redo) } {
        return false;
    }
    record(ops, host, name, &[undo], &[redo], false);
    true
}

#[path = "region_geometry_edits.rs"]
mod geometry_edits;
pub use geometry_edits::*;

#[path = "region_comp_edits.rs"]
mod comp_edits;
pub use comp_edits::*;

fn edit_remove_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> bool {
    if track_id == 0 || region_id == 0 {
        return false;
    }
    let (Some(find), Some(apply), Some(snapshot), Some(release)) = (
        ops.find_region,
        ops.apply_edit,
        ops.snapshot_region,
        ops.release_snapshot,
    ) else {
        return false;
    };
    let mut state = HirariRegionState::default();
    if !unsafe { find(host, track_id, region_id, &mut state) } {
        return false;
    }
    let handle = unsafe { snapshot(host, track_id, region_id) };
    if handle.is_null() {
        return false;
    }
    let remove = state_payload(track_id, region_id, HIRARI_REGION_EDIT_REMOVE, state);
    if !unsafe { apply(host, &remove) } {
        unsafe { release(host, handle) };
        return false;
    }
    let mut restore = state_payload(track_id, region_id, HIRARI_REGION_EDIT_RESTORE, state);
    restore.blob = handle as *const u8;
    if can_record(ops) {
        // The host takes ownership of the handle with the undo entry.
        record(ops, host, b"Remove Region\0", &[restore], &[remove], false);
    } else {
        unsafe { release(host, handle) };
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_remove(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    edit_remove_with(&ops, host, track_id, region_id)
}

/// Keeps a decoded handle alive until it is handed to the host.
struct DecodedAudio {
    ops: HirariRegionHostOps,
    handle: *mut c_void,
}

impl DecodedAudio {
    fn into_raw(self) -> *mut c_void {
        let handle = self.handle;
        std::mem::forget(self);
        handle
    }
}

impl Drop for DecodedAudio {
    fn drop(&mut self) {
        if let Some(release) = self.ops.release_audio {
            unsafe { release(self.handle) };
        }
    }
}

fn audio_rate_supported(rate: f64) -> bool {
    rate.is_finite() && (8_000.0..=384_000.0).contains(&rate)
}

fn restore_payload(
    track_id: u32,
    region_id: u32,
    snapshot: *mut c_void,
) -> HirariRegionEditPayload {
    let mut payload = state_payload(
        track_id,
        region_id,
        HIRARI_REGION_EDIT_RESTORE,
        HirariRegionState::default(),
    );
    payload.blob = snapshot as *const u8;
    payload
}

/// Decodes `path` and swaps it into the region, recording one undo entry.
///
/// The native setter rejected an empty path, an unreadable file and rates
/// outside 8 kHz..384 kHz, and snapshotted the region before and after the
/// swap; all of that lives here now. The host still performs the swap and
/// publishes the snapshot.
fn replace_audio_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    path: &[u8],
) -> bool {
    if path.is_empty() {
        return false;
    }
    let (Some(find), Some(apply), Some(snapshot)) = (
        ops.find_region,
        ops.apply_edit,
        ops.snapshot_region,
    ) else {
        return false;
    };
    let Some(read_rate) = ops.sample_rate else {
        return false;
    };
    // Decode before the lookup, as the native setter did, so an unreadable
    // file fails the same way whether or not the region exists.
    let Some((handle, source_sample_rate, _, decoded_samples)) =
        crate::audio_import_cache::import_audio_file(path)
    else {
        return false;
    };
    let audio = DecodedAudio { ops: *ops, handle };
    let mut state = HirariRegionState::default();
    if !unsafe { find(host, track_id, region_id, &mut state) } {
        return false;
    }
    let timeline_sample_rate = unsafe { read_rate(host) };
    if !audio_rate_supported(source_sample_rate) || !audio_rate_supported(timeline_sample_rate) {
        return false;
    }
    let before = unsafe { snapshot(host, track_id, region_id) };
    if before.is_null() {
        return false;
    }
    let release_snapshot = |handle: *mut c_void| {
        if let Some(release) = ops.release_snapshot {
            unsafe { release(host, handle) };
        }
    };
    // The decoded length and the source/timeline mapping give the replacement
    // its timeline geometry, so the native setter only stores what is resolved
    // here. Rates are installed first: the mapping is derived from them.
    state.source_sample_rate = source_sample_rate;
    state.timeline_sample_rate = timeline_sample_rate;
    let frames_per_timeline = state.source_frames_per_timeline_frame();
    let length = (decoded_samples as f64 / frames_per_timeline).ceil();
    if !length.is_finite() || length < 1.0 || length >= u64::MAX as f64 {
        return false;
    }
    state.source_offset = 0;
    state.base_source_offset = 0;
    state.source_length = decoded_samples;
    state.base_start = state.start;
    state.base_length = decoded_samples;
    state.len = length as u64;
    let mut blob = Vec::with_capacity(REPLACE_AUDIO_HEADER_BYTES + path.len());
    blob.extend_from_slice(&(audio.into_raw() as usize).to_ne_bytes());
    blob.extend_from_slice(path);
    let payload = HirariRegionEditPayload {
        track_id,
        region_id,
        op: HIRARI_REGION_EDIT_REPLACE_AUDIO,
        blob: blob.as_ptr(),
        blob_len: blob.len(),
        state,
        ..Default::default()
    };
    if !unsafe { apply(host, &payload) } {
        release_snapshot(before);
        return false;
    }
    let after = unsafe { snapshot(host, track_id, region_id) };
    if after.is_null() {
        release_snapshot(before);
        return false;
    }
    if can_record(ops) {
        // Both handles move to the undo entry.
        record(
            ops,
            host,
            b"Replace Region Audio\0",
            &[restore_payload(track_id, region_id, before)],
            &[restore_payload(track_id, region_id, after)],
            false,
        );
    } else {
        release_snapshot(before);
        release_snapshot(after);
    }
    true
}

/// Replaces a region's audio from a file path.
///
/// # Safety
/// `host` must be the opaque handle the installed host ops expect, the
/// installed table must outlive the call, and `path` must be readable for
/// `path_length` bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_replace_audio(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    path: *const u8,
    path_length: usize,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    if path.is_null() && path_length != 0 {
        return false;
    }
    let path = if path_length == 0 {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(path, path_length) }
    };
    replace_audio_with(&ops, host, track_id, region_id, path)
}

fn read_comp_ranges(
    read: HirariRegionCompRangesFn,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> Option<(Vec<NativeCompRange>, bool)> {
    let mut managed = false;
    let required = unsafe { read(host, track_id, region_id, ptr::null_mut(), 0, &mut managed) };
    if required > 4096 {
        return None;
    }
    let mut ranges = vec![NativeCompRange::default(); required];
    if required == 0 {
        return Some((ranges, managed));
    }
    let actual = unsafe {
        read(
            host,
            track_id,
            region_id,
            ranges.as_mut_ptr(),
            required,
            &mut managed,
        )
    };
    (actual == required).then_some((ranges, managed))
}

pub unsafe extern "C" fn hirari_region_edit_scalar(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    field: i32,
    value_a: f32,
    value_b: f32,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    edit_scalar_with(&ops, host, track_id, region_id, field, value_a, value_b)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_move(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    start: f64,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    edit_move_with(&ops, host, track_id, region_id, start)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_move_sync_group(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    start: f64,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    edit_move_sync_group_with(&ops, host, track_id, region_id, start)
}


/// Payload op: restore the note table from an owned Rust snapshot handle.
pub const HIRARI_REGION_EDIT_NOTE_STORE: i32 = 11;

/// One note-table mutation plus the snapshots the trampoline collects while
/// the host holds the region lock.

#[path = "region_edit_host_notes.rs"]
mod note_edits;
pub(in crate::region_edit_host) use note_edits::*;
pub use note_edits::{
    hirari_region_edit_note_segment, hirari_region_edit_note_move_anchor,
    hirari_region_edit_note_anchor, hirari_region_edit_note_pitch_anchor,
    hirari_region_edit_note_formant_anchor, hirari_region_edit_note_remove,
    hirari_region_edit_note_clear, hirari_region_edit_note_warp,
    hirari_region_note_store_restore,
};

#[cfg(test)]
#[path = "region_edit_host_tests.rs"]
mod tests;

#[path = "region_split_host.rs"]
mod split_host;
#[cfg(test)]
pub(in crate::region_edit_host) use split_host::*;
pub use split_host::{hirari_region_split, hirari_region_duplicate};

