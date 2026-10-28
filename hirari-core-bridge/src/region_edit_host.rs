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
/// Payload op: swap in a decoded audio handle. The blob starts with a
/// native-layout header (handle, source rate, timeline rate) followed by the
/// path bytes.
pub const HIRARI_REGION_EDIT_REPLACE_AUDIO: i32 = 18;
const REPLACE_AUDIO_HEADER_BYTES: usize = 8 + 2 * 8;
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

impl HirariRegionState {
    /// Matches `Region::sourceFramesPerTimelineFrame()` in `track.hpp`.
    pub(crate) fn source_frames_per_timeline_frame(&self) -> f64 {
        let source_rate = if self.source_sample_rate.is_finite() && self.source_sample_rate > 0.0 {
            self.source_sample_rate
        } else {
            self.timeline_sample_rate
        };
        let timeline_rate = if self.timeline_sample_rate.is_finite() && self.timeline_sample_rate > 0.0
        {
            self.timeline_sample_rate
        } else {
            source_rate
        };
        let conversion = source_rate / timeline_rate;
        let effective = self.warp_ratio * conversion;
        if effective.is_finite() && effective > 0.0 {
            effective
        } else {
            self.warp_ratio
        }
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
/// Decodes a media file through the native decoder and returns an owned audio
/// handle plus the source sample rate captured during the import. The handle
/// is read through `HirariRegionAudioViewFn` and freed by
/// `HirariRegionReleaseAudioFn`.
pub type HirariRegionDecodeAudioFn = unsafe extern "C" fn(
    *mut c_void,
    *const u8,
    usize,
    *mut *mut c_void,
    *mut f64,
) -> bool;
/// Inserts a freshly decoded region into the project. The audio handle is
/// consumed on every path. Returns the new region id, or 0 on failure.
pub type HirariRegionInsertFn = unsafe extern "C" fn(
    *mut c_void,
    u32,
    usize,
    *const u8,
    usize,
    u64,
    f64,
    f64,
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
/// Runs the analysis kernel on a retained track.
pub type HirariRegionPitchAnalyzeFn =
    unsafe extern "C" fn(*mut c_void, u32, f64) -> bool;
/// Ends F0 analysis ownership on a retained track.
pub type HirariRegionPitchFinishFn = unsafe extern "C" fn(*mut c_void);
/// Copies a region's note table from a retained track into an owned store.
pub type HirariRegionNoteSnapshotFn =
    unsafe extern "C" fn(*mut c_void, u32) -> *mut c_void;
/// Clones a region under a fresh id, optionally replacing its editable state
/// first, and adds the copy. Returns the new id, or 0 on failure.
pub type HirariRegionCloneFn =
    unsafe extern "C" fn(*mut c_void, u32, u32, *const HirariRegionState) -> u32;
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
    pub decode_audio: Option<HirariRegionDecodeAudioFn>,
    pub insert_region: Option<HirariRegionInsertFn>,
    pub track_identity: Option<HirariRegionTrackIdentityFn>,
    pub retain_track: Option<HirariRegionRetainTrackFn>,
    pub release_track: Option<HirariRegionReleaseTrackFn>,
    pub pitch_begin: Option<HirariRegionPitchBeginFn>,
    pub pitch_analyze: Option<HirariRegionPitchAnalyzeFn>,
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

/// Installs the native host callbacks. Passing null clears them.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_host_install(ops: *const HirariRegionHostOps) {
    let slot = HOST_OPS.get_or_init(|| Mutex::new(None));
    let value = if ops.is_null() {
        None
    } else {
        Some(unsafe { ptr::read(ops) })
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

/// Applies the accepted decision to a copy of the current state. Only the
/// fields the policy controls are touched; geometry is left alone.
fn edited_state(
    state: HirariRegionState,
    field: HirariRegionScalarField,
    decision: &HirariRegionScalarDecision,
    raw_a: f32,
) -> HirariRegionState {
    let mut after = state;
    match field {
        HirariRegionScalarField::ClipGain => after.clip_gain = decision.value_a,
        HirariRegionScalarField::Muted => after.muted = raw_a != 0.0,
        HirariRegionScalarField::Reverse => after.reverse = raw_a != 0.0,
        HirariRegionScalarField::PitchSemitones => after.pitch_semitones = decision.value_a,
        HirariRegionScalarField::LoopCount => after.loop_count = decision.value_a.to_bits(),
        HirariRegionScalarField::SyncGroup => after.sync_group = decision.value_a.to_bits(),
        HirariRegionScalarField::Locked => after.locked = raw_a != 0.0,
        HirariRegionScalarField::PitchPreserveWarp => {
            // The C++ setter backfilled the untrimmed source length before the
            // flag could be honoured by the renderer.
            if after.source_length == 0 {
                after.source_length = after.len;
            }
            after.pitch_preserve_warp = raw_a != 0.0;
        }
        HirariRegionScalarField::Fades => {
            after.fade_in_samples =
                unsafe { hirari_region_fade_samples(decision.value_a, state.len) };
            after.fade_out_samples =
                unsafe { hirari_region_fade_samples(decision.value_b, state.len) };
        }
    }
    after
}

pub(crate) fn record(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    name: &[u8],
    undo: &[HirariRegionEditPayload],
    redo: &[HirariRegionEditPayload],
    coalesce: bool,
) -> bool {
    record_with(ops, host, name, undo, redo, coalesce, false)
}

/// Records an edit the host must replay through the redo callback before the
/// history entry exists, matching the native `performAction` contract.
fn record_performed(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    name: &[u8],
    undo: &[HirariRegionEditPayload],
    redo: &[HirariRegionEditPayload],
) -> bool {
    record_with(ops, host, name, undo, redo, true, true)
}

fn record_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    name: &[u8],
    undo: &[HirariRegionEditPayload],
    redo: &[HirariRegionEditPayload],
    coalesce: bool,
    perform: bool,
) -> bool {
    match ops.record_undo {
        Some(record_undo) => unsafe {
            record_undo(
                host,
                name.as_ptr() as *const c_char,
                undo.as_ptr(),
                undo.len(),
                redo.as_ptr(),
                redo.len(),
                coalesce,
                perform,
            )
        },
        None => false,
    }
}

/// Applies one scalar field edit through an explicit host table.
///
/// Kept separate from the exported function so tests never have to touch the
/// process-global table while other tests are driving the native engine.
fn edit_scalar_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    field: i32,
    value_a: f32,
    value_b: f32,
) -> bool {
    let (Some(find), Some(apply)) = (ops.find_region, ops.apply_edit) else {
        return false;
    };
    let Some(field) = scalar_field(field) else {
        return false;
    };
    let mut state = HirariRegionState::default();
    if !unsafe { find(host, track_id, region_id, &mut state) } {
        return false;
    }
    // The engine-level precheck ignores the lock; the region's own lock state
    // is enforced by the storage layer when it writes the fields.
    let edit = HirariRegionScalarEdit {
        field,
        locked: state.locked,
        value_a,
        value_b,
        region_len: state.len,
        region_start: state.start,
    };
    let decision = HirariRegionScalarDecision::decide(&edit);
    if !decision.accepted {
        return false;
    }
    let after = edited_state(state, field, &decision, value_a);
    let before_payload = state_payload(track_id, region_id, HIRARI_REGION_EDIT_INPLACE, state);
    let after_payload = state_payload(track_id, region_id, HIRARI_REGION_EDIT_INPLACE, after);
    if !unsafe { apply(host, &after_payload) } {
        return false;
    }
    if let Some(name) = field_name(field) {
        record(ops, host, name, &[before_payload], &[after_payload], true);
    }
    true
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

fn edit_trim_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    start_normalized: f32,
    end_normalized: f32,
) -> bool {
    if !start_normalized.is_finite() || !end_normalized.is_finite() {
        return false;
    }
    let start_normalized = start_normalized.clamp(0.0, 1.0);
    let end_normalized = end_normalized.clamp(0.0, 1.0);
    if end_normalized <= start_normalized {
        return false;
    }
    let Some(find) = ops.find_region else {
        return false;
    };
    let mut state = HirariRegionState::default();
    if !unsafe { find(host, track_id, region_id, &mut state) } {
        return false;
    }
    // The previous engine wrapper only offered trim to regions with a base
    // (untrimmed) length, and the storage layer refused locked regions.
    if state.locked || state.base_length == 0 {
        return false;
    }
    let input = geometry_of(state);
    let mut output = NativeRegionGeometry::default();
    if !unsafe {
        hirari_region_set_trim_geometry(&input, start_normalized, end_normalized, &mut output)
    } {
        return false;
    }
    commit_geometry(ops, host, track_id, region_id, state, output, b"Trim Region\0")
}

fn edit_warp_ratio_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    ratio: f64,
) -> bool {
    if !ratio.is_finite() || !(0.5..=2.0).contains(&ratio) {
        return false;
    }
    let Some(find) = ops.find_region else {
        return false;
    };
    let mut state = HirariRegionState::default();
    if !unsafe { find(host, track_id, region_id, &mut state) } {
        return false;
    }
    if state.locked {
        return false;
    }
    let input = geometry_of(state);
    let mut output = NativeRegionGeometry::default();
    if !unsafe { hirari_region_set_warp_ratio_geometry(&input, ratio, &mut output) } {
        return false;
    }
    commit_geometry(ops, host, track_id, region_id, state, output, b"Flex Warp Ratio\0")
}

/// Recomputes and applies a normalized trim. The engine keeps region storage;
/// this owns the bounds policy, the geometry math and the undo capture.
///
/// # Safety
/// `host` must be the opaque handle the installed host ops expect, and the
/// installed table must outlive the call.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_trim(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    start_normalized: f32,
    end_normalized: f32,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    edit_trim_with(
        &ops,
        host,
        track_id,
        region_id,
        start_normalized,
        end_normalized,
    )
}

/// Recomputes and applies a warp-ratio edit, clearing the region's markers.
///
/// # Safety
/// `host` must be the opaque handle the installed host ops expect, and the
/// installed table must outlive the call.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_warp_ratio(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    ratio: f64,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    edit_warp_ratio_with(&ops, host, track_id, region_id, ratio)
}

/// Converts the bridge's three parallel frames (source samples, timeline
/// samples, transient flags) into the marker layout and forwards them to
/// [`set_warp_markers_with`]. Disagreeing lengths and an oversized list are
/// rejected before any conversion, so a rejected request allocates nothing.
fn set_warp_markers_from_slices(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    source_samples: *const u64,
    source_count: usize,
    timeline_samples: *const u64,
    timeline_count: usize,
    transient_flags: *const u8,
    flag_count: usize,
) -> bool {
    if source_count != timeline_count || source_count != flag_count {
        return false;
    }
    if source_count == 0 {
        return set_warp_markers_with(ops, host, track_id, region_id, ptr::null(), 0);
    }
    if source_count > MAX_WARP_MARKER_EDITS
        || source_samples.is_null()
        || timeline_samples.is_null()
        || transient_flags.is_null()
    {
        return false;
    }
    let source = unsafe { std::slice::from_raw_parts(source_samples, source_count) };
    let timeline = unsafe { std::slice::from_raw_parts(timeline_samples, source_count) };
    let flags = unsafe { std::slice::from_raw_parts(transient_flags, source_count) };
    let markers: Vec<HirariWarpMarker> = (0..source_count)
        .map(|index| HirariWarpMarker {
            source_sample: source[index],
            timeline_sample: timeline[index],
            transient: u8::from(flags[index] != 0),
        })
        .collect();
    set_warp_markers_with(ops, host, track_id, region_id, markers.as_ptr(), markers.len())
}

/// Replaces a region's warp markers from the bridge's three parallel slices.
///
/// # Safety
/// `host` must be the opaque handle the installed host ops expect, and every
/// non-null slice must point to its declared element count for the duration of
/// the call.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_set_warp_markers(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    source_samples: *const u64,
    source_count: usize,
    timeline_samples: *const u64,
    timeline_count: usize,
    transient_flags: *const u8,
    flag_count: usize,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    set_warp_markers_from_slices(
        &ops,
        host,
        track_id,
        region_id,
        source_samples,
        source_count,
        timeline_samples,
        timeline_count,
        transient_flags,
        flag_count,
    )
}

fn read_range_edits(
    read: HirariRegionRangeEditsFn,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> Option<Vec<NativeRangeEdit>> {
    let required = unsafe { read(host, track_id, region_id, ptr::null_mut(), 0) };
    let mut edits = vec![NativeRangeEdit::default(); required];
    if required == 0 {
        return Some(edits);
    }
    let actual = unsafe { read(host, track_id, region_id, edits.as_mut_ptr(), required) };
    (actual == required).then_some(edits)
}

fn range_edit_payload(
    track_id: u32,
    region_id: u32,
    state: HirariRegionState,
    edits: &[NativeRangeEdit],
) -> HirariRegionEditPayload {
    HirariRegionEditPayload {
        track_id,
        region_id,
        op: HIRARI_REGION_EDIT_RANGE_EDITS,
        blob: if edits.is_empty() {
            ptr::null()
        } else {
            edits.as_ptr().cast()
        },
        blob_len: std::mem::size_of_val(edits),
        state,
        ..Default::default()
    }
}

/// Applies one range-edit upsert. The Rust transform owns the bounds, gain and
/// fade rules; the undo entry is the pair of stored lists, replayed through the
/// host's perform path exactly like the native setter did.
fn set_range_edit_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    start: u64,
    end: u64,
    gain: f32,
    fade_in: u64,
    fade_out: u64,
) -> bool {
    let (Some(find), Some(apply), Some(read)) =
        (ops.find_region, ops.apply_edit, ops.range_edits)
    else {
        return false;
    };
    let mut state = HirariRegionState::default();
    if !unsafe { find(host, track_id, region_id, &mut state) } || state.locked {
        return false;
    }
    let Some(before) = read_range_edits(read, host, track_id, region_id) else {
        return false;
    };
    let mut produced = 0usize;
    let result = unsafe {
        hirari_region_range_edits_apply(
            before.as_ptr(),
            before.len(),
            state.len,
            1, // upsert
            start,
            end,
            gain,
            fade_in,
            fade_out,
            &mut produced,
        )
    };
    if result.is_null() {
        return false;
    }
    let after = unsafe { slice::from_raw_parts(result, produced) }.to_vec();
    unsafe { hirari_free_range_edits(result, produced) };
    if !unsafe { apply(host, &range_edit_payload(track_id, region_id, state, &after)) } {
        return false;
    }
    record_performed(
        ops,
        host,
        b"Region Range Edit\0",
        &[range_edit_payload(track_id, region_id, state, &before)],
        &[range_edit_payload(track_id, region_id, state, &after)],
    );
    true
}

fn clear_range_edits_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> bool {
    let (Some(find), Some(apply), Some(read)) =
        (ops.find_region, ops.apply_edit, ops.range_edits)
    else {
        return false;
    };
    let mut state = HirariRegionState::default();
    if !unsafe { find(host, track_id, region_id, &mut state) } || state.locked {
        return false;
    }
    let Some(before) = read_range_edits(read, host, track_id, region_id) else {
        return false;
    };
    if before.is_empty() {
        return false;
    }
    if !unsafe { apply(host, &range_edit_payload(track_id, region_id, state, &[])) } {
        return false;
    }
    record_performed(
        ops,
        host,
        b"Clear Region Range Edits\0",
        &[range_edit_payload(track_id, region_id, state, &before)],
        &[range_edit_payload(track_id, region_id, state, &[])],
    );
    true
}

/// Adds or replaces the range edit covering `[start, end)`.
///
/// # Safety
/// `host` must be the opaque handle the installed host ops expect, and the
/// installed table must outlive the call.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_range_edit_set(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    start: u64,
    end: u64,
    gain: f32,
    fade_in: u64,
    fade_out: u64,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    set_range_edit_with(
        &ops, host, track_id, region_id, start, end, gain, fade_in, fade_out,
    )
}

/// Removes every range edit from one region, recording the previous list.
///
/// # Safety
/// `host` must be the opaque handle the installed host ops expect, and the
/// installed table must outlive the call.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_range_edits_clear(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    clear_range_edits_with(&ops, host, track_id, region_id)
}

/// Replaces a region's comp-range batch.
///
/// The policy (count, ordering, fades, bounds, lock/managed interaction) is
/// `hirari_region_comp_ranges_validate`; the host only stores the accepted
/// batch. The native setter never touched the undo history, so neither does
/// this path.
fn set_comp_ranges_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    packed_words: *const u64,
    word_count: usize,
    managed: bool,
) -> bool {
    let (Some(find), Some(apply)) = (ops.find_region, ops.apply_edit) else {
        return false;
    };
    if word_count % 4 != 0 || word_count / 4 > 4096 {
        return false;
    }
    let mut state = HirariRegionState::default();
    if !unsafe { find(host, track_id, region_id, &mut state) } {
        return false;
    }
    let range_count = word_count / 4;
    if !unsafe {
        hirari_region_comp_ranges_validate(
            packed_words.cast::<NativeCompRange>(),
            range_count,
            state.len,
            state.locked,
            managed,
        )
    } {
        return false;
    }
    let payload = HirariRegionEditPayload {
        track_id,
        region_id,
        op: HIRARI_REGION_EDIT_COMP_RANGES,
        blob: if word_count == 0 {
            ptr::null()
        } else {
            packed_words.cast()
        },
        blob_len: word_count * std::mem::size_of::<u64>(),
        state,
        managed,
    };
    unsafe { apply(host, &payload) }
}

/// Adopts a comp-range batch packed as four `u64` words per range.
///
/// # Safety
/// `host` must be the opaque handle the installed host ops expect, the
/// installed table must outlive the call, and `packed_words` must be readable
/// for `word_count` words.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_comp_ranges_set(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    packed_words: *const u64,
    word_count: usize,
    managed: bool,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    set_comp_ranges_with(
        &ops,
        host,
        track_id,
        region_id,
        packed_words,
        word_count,
        managed,
    )
}

/// Moves one region to `start` through an explicit host table.
fn edit_move_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    start: f64,
) -> bool {
    if !start.is_finite() || start < 0.0 || start > u64::MAX as f64 {
        return false;
    }
    let (Some(find), Some(apply)) = (ops.find_region, ops.apply_edit) else {
        return false;
    };
    let mut state = HirariRegionState::default();
    if !unsafe { find(host, track_id, region_id, &mut state) } {
        return false;
    }
    let new_start = start as u64;
    let source_trim = if state.source_offset >= state.base_source_offset {
        state.source_offset - state.base_source_offset
    } else {
        0
    };
    let factor = state.source_frames_per_timeline_frame();
    let quotient = source_trim as f64 / factor;
    if !quotient.is_finite() {
        return false;
    }
    let timeline_trim = quotient.ceil();
    if timeline_trim < 0.0 || timeline_trim > new_start as f64 {
        return false;
    }
    let mut after = state;
    after.start = new_start;
    after.base_start = new_start - timeline_trim as u64;
    let before = state_payload(track_id, region_id, HIRARI_REGION_EDIT_STATE, state);
    let redo = state_payload(track_id, region_id, HIRARI_REGION_EDIT_STATE, after);
    if !unsafe { apply(host, &redo) } {
        return false;
    }
    record(ops, host, b"Move Region\0", &[before], &[redo], true);
    true
}

/// Moves every region in the anchor's sync group through an explicit host
/// table.
fn edit_move_sync_group_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    start: f64,
) -> bool {
    if track_id == 0
        || region_id == 0
        || !start.is_finite()
        || start < 0.0
        || start > u64::MAX as f64
    {
        return false;
    }
    let (Some(find), Some(apply), Some(list)) =
        (ops.find_region, ops.apply_edit, ops.list_sync_group)
    else {
        return false;
    };
    let mut anchor_state = HirariRegionState::default();
    if !unsafe { find(host, track_id, region_id, &mut anchor_state) } {
        return false;
    }
    let group = anchor_state.sync_group;
    if group == 0 || anchor_state.locked {
        return false;
    }
    let required = unsafe { list(host, group, ptr::null_mut(), 0) };
    if required < 2 {
        return false;
    }
    let mut members = vec![HirariRegionGroupMember::default(); required];
    let written = unsafe { list(host, group, members.as_mut_ptr(), required) };
    if written != required || members.iter().any(|member| member.locked) {
        return false;
    }
    let Some(anchor) = members
        .iter()
        .find(|member| member.track_id == track_id && member.region_id == region_id)
    else {
        return false;
    };
    let starts: Vec<u64> = members.iter().map(|member| member.start).collect();
    let base_starts: Vec<u64> = members.iter().map(|member| member.base_start).collect();
    let mut moved_starts = vec![0u64; required];
    let mut moved_base_starts = vec![0u64; required];
    let new_start = start as u64;
    if !unsafe {
        hirari_region_sync_move_positions(
            starts.as_ptr(),
            base_starts.as_ptr(),
            required,
            anchor.start,
            new_start,
            moved_starts.as_mut_ptr(),
            moved_base_starts.as_mut_ptr(),
        )
    } {
        return false;
    }
    let mut undo = Vec::with_capacity(required);
    let mut redo = Vec::with_capacity(required);
    for (index, member) in members.iter().enumerate() {
        let mut state = HirariRegionState::default();
        if !unsafe { find(host, member.track_id, member.region_id, &mut state) } {
            return false;
        }
        let mut after = state;
        after.start = moved_starts[index];
        after.base_start = moved_base_starts[index];
        undo.push(state_payload(member.track_id, member.region_id, HIRARI_REGION_EDIT_STATE, state));
        redo.push(state_payload(member.track_id, member.region_id, HIRARI_REGION_EDIT_STATE, after));
    }
    for payload in &redo {
        // `addRegion` normalisation can reject a single member; the original
        // C++ loop ignored apply failures too, so the group stays consistent.
        unsafe { apply(host, payload) };
    }
    record(ops, host, b"Move Region Sync Group\0", &undo, &redo, true);
    true
}

/// Removes one region through an explicit host table. Undo restores the
/// whole region from a snapshot the host holds, because audio buffers and
/// processing history cannot travel in a numeric payload.
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
    if ops.record_undo.is_some() {
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
    let (Some(find), Some(apply), Some(decode), Some(snapshot)) = (
        ops.find_region,
        ops.apply_edit,
        ops.decode_audio,
        ops.snapshot_region,
    ) else {
        return false;
    };
    let Some(read_rate) = ops.sample_rate else {
        return false;
    };
    // Decode before the lookup, as the native setter did, so an unreadable
    // file fails the same way whether or not the region exists.
    let mut handle = ptr::null_mut();
    let mut source_sample_rate = 0.0;
    if !unsafe { decode(host, path.as_ptr(), path.len(), &mut handle, &mut source_sample_rate) }
        || handle.is_null()
    {
        return false;
    }
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
    let mut blob = Vec::with_capacity(REPLACE_AUDIO_HEADER_BYTES + path.len());
    blob.extend_from_slice(&(audio.into_raw() as usize).to_ne_bytes());
    blob.extend_from_slice(&source_sample_rate.to_ne_bytes());
    blob.extend_from_slice(&timeline_sample_rate.to_ne_bytes());
    blob.extend_from_slice(path);
    let payload = HirariRegionEditPayload {
        track_id,
        region_id,
        op: HIRARI_REGION_EDIT_REPLACE_AUDIO,
        blob: blob.as_ptr(),
        blob_len: blob.len(),
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
    if ops.record_undo.is_some() {
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

/// Owns a split plan's buffers until the request finishes.
struct SplitPlan(HirariRegionSplitOutput);

impl Drop for SplitPlan {
    fn drop(&mut self) {
        unsafe { hirari_region_split_plan_free(&mut self.0) };
    }
}

/// Borrows a split plan's boxed array; empty plans use null pointers.
///
/// # Safety
/// `ptr` must come from the matching plan field and stay alive for `'a`.
unsafe fn plan_slice<'a, T>(ptr: *mut T, count: usize) -> &'a [T] {
    if count == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(ptr, count) }
    }
}

/// Writes one side of a split through the payload ops: editable state, warp
/// markers, range edits, comp ranges and the windowed note table.
fn apply_split_side(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    state: HirariRegionState,
    markers: &[HirariWarpMarker],
    ranges: &[NativeRangeEdit],
    comp_ranges: &[NativeCompRange],
    comp_managed: bool,
    notes: *mut HirariRegionVariAudioStore,
) -> bool {
    let Some(apply) = ops.apply_edit else {
        return false;
    };
    let payloads = [
        state_payload(track_id, region_id, HIRARI_REGION_EDIT_STATE, state),
        HirariRegionEditPayload {
            track_id,
            region_id,
            op: HIRARI_REGION_EDIT_MARKERS,
            blob: markers.as_ptr().cast(),
            blob_len: std::mem::size_of_val(markers),
            ..Default::default()
        },
        HirariRegionEditPayload {
            track_id,
            region_id,
            op: HIRARI_REGION_EDIT_RANGE_EDITS,
            blob: ranges.as_ptr().cast(),
            blob_len: std::mem::size_of_val(ranges),
            ..Default::default()
        },
        HirariRegionEditPayload {
            track_id,
            region_id,
            op: HIRARI_REGION_EDIT_COMP_RANGES,
            blob: comp_ranges.as_ptr().cast(),
            blob_len: std::mem::size_of_val(comp_ranges),
            managed: comp_managed,
            ..Default::default()
        },
        note_store_payload(track_id, region_id, notes),
    ];
    payloads.iter().all(|payload| unsafe { apply(host, payload) })
}

/// Splits one region at an absolute timeline sample position.
///
/// The native splitter validated the position, built both payloads, split the
/// note table and recorded the undo pair in C++. All of that lives here now;
/// the host only clones and stores regions.
fn split_region_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    split_beat: f64,
) -> u32 {
    // The bridge passes an absolute sample position as a double. Reject
    // fractional positions instead of truncating them.
    if !split_beat.is_finite() || split_beat < 0.0 || split_beat.floor() != split_beat {
        return 0;
    }
    let (Some(find), Some(apply), Some(snapshot)) =
        (ops.find_region, ops.apply_edit, ops.snapshot_region)
    else {
        return 0;
    };
    let (Some(release_snapshot), Some(clone)) = (ops.release_snapshot, ops.clone_region) else {
        return 0;
    };
    let (Some(read_ranges), Some(read_comp)) = (ops.range_edits, ops.comp_ranges) else {
        return 0;
    };
    let (Some(retain_track), Some(release_track), Some(note_snapshot)) =
        (ops.retain_track, ops.release_track, ops.note_snapshot)
    else {
        return 0;
    };
    let Some(read_rate) = ops.sample_rate else {
        return 0;
    };
    let mut state = HirariRegionState::default();
    if !unsafe { find(host, track_id, region_id, &mut state) } {
        return 0;
    }
    if state.len == 0 || state.start.checked_add(state.len).is_none() {
        return 0;
    }
    let end = state.start as f64 + state.len as f64;
    if split_beat <= state.start as f64 || split_beat >= end {
        return 0;
    }
    let split_offset = (split_beat - state.start as f64) as u64;
    if split_offset == 0 || split_offset >= state.len {
        return 0;
    }
    let Some(markers) = warp_markers_of(ops, host, track_id, region_id) else {
        return 0;
    };
    let Some(ranges) = read_range_edits(read_ranges, host, track_id, region_id) else {
        return 0;
    };
    let Some((comp_ranges, comp_managed)) = read_comp_ranges(read_comp, host, track_id, region_id)
    else {
        return 0;
    };
    let input = HirariRegionSplitInput {
        state,
        split_offset,
        markers: markers.as_ptr(),
        marker_count: markers.len(),
        ranges: ranges.as_ptr(),
        range_count: ranges.len(),
        comp_ranges: comp_ranges.as_ptr(),
        comp_range_count: comp_ranges.len(),
    };
    let mut plan = SplitPlan(HirariRegionSplitOutput::default());
    if !unsafe { hirari_region_split_plan(&input, &mut plan.0) } {
        return 0;
    }

    // Pitch/formant note edits use region-relative seconds: window each
    // segment at the split and move the right half back to time zero. The
    // retained-track snapshot reads the published projection, so a concurrent
    // note edit cannot mutate the table while it is windowed.
    let track_handle = unsafe { retain_track(host, track_id) };
    if track_handle.is_null() {
        return 0;
    }
    let source_notes = unsafe { note_snapshot(track_handle, region_id) };
    unsafe { release_track(track_handle) };
    if source_notes.is_null() {
        return 0;
    }
    let left_notes = unsafe { hirari_region_vari_audio_store_new() };
    let right_notes = unsafe { hirari_region_vari_audio_store_new() };
    let notes_split = !left_notes.is_null()
        && !right_notes.is_null()
        && unsafe {
            hirari_region_vari_audio_store_split(
                source_notes.cast(),
                left_notes,
                right_notes,
                read_rate(host),
                split_offset,
                state.len,
            )
        };
    unsafe {
        vari_store_release(source_notes.cast());
        if !notes_split {
            vari_store_release(left_notes);
            vari_store_release(right_notes);
        }
    }
    if !notes_split {
        return 0;
    }

    let before = unsafe { snapshot(host, track_id, region_id) };
    if before.is_null() {
        unsafe {
            vari_store_release(left_notes);
            vari_store_release(right_notes);
        }
        return 0;
    }
    let right_id = unsafe { clone(host, track_id, region_id, ptr::null()) };
    if right_id == 0 {
        unsafe {
            release_snapshot(host, before);
            vari_store_release(left_notes);
            vari_store_release(right_notes);
        }
        return 0;
    }

    let left_ok = apply_split_side(
        ops,
        host,
        track_id,
        region_id,
        plan.0.left,
        unsafe { plan_slice(plan.0.left_markers, plan.0.left_marker_count) },
        unsafe { plan_slice(plan.0.left_ranges, plan.0.left_range_count) },
        unsafe { plan_slice(plan.0.left_comp_ranges, plan.0.left_comp_range_count) },
        comp_managed,
        left_notes,
    );
    let right_ok = left_ok
        && apply_split_side(
            ops,
            host,
            track_id,
            right_id,
            plan.0.right,
            unsafe { plan_slice(plan.0.right_markers, plan.0.right_marker_count) },
            unsafe { plan_slice(plan.0.right_ranges, plan.0.right_range_count) },
            unsafe { plan_slice(plan.0.right_comp_ranges, plan.0.right_comp_range_count) },
            comp_managed,
            right_notes,
        );
    unsafe {
        vari_store_release(left_notes);
        vari_store_release(right_notes);
    }
    if !right_ok {
        // The region was already mutated, so put the original back and drop
        // the clone instead of leaving a half-applied split behind.
        let restore = restore_payload(track_id, region_id, before);
        let remove = state_payload(
            track_id,
            right_id,
            HIRARI_REGION_EDIT_REMOVE,
            HirariRegionState::default(),
        );
        unsafe {
            apply(host, &restore);
            apply(host, &remove);
            release_snapshot(host, before);
        }
        return 0;
    }

    let left_snapshot = unsafe { snapshot(host, track_id, region_id) };
    let right_snapshot = unsafe { snapshot(host, track_id, right_id) };
    if left_snapshot.is_null() || right_snapshot.is_null() {
        if !left_snapshot.is_null() {
            unsafe { release_snapshot(host, left_snapshot) };
        }
        if !right_snapshot.is_null() {
            unsafe { release_snapshot(host, right_snapshot) };
        }
        let restore = restore_payload(track_id, region_id, before);
        let remove = state_payload(
            track_id,
            right_id,
            HIRARI_REGION_EDIT_REMOVE,
            HirariRegionState::default(),
        );
        unsafe {
            apply(host, &restore);
            apply(host, &remove);
            release_snapshot(host, before);
        }
        return 0;
    }
    if ops.record_undo.is_some() {
        let undo = [
            restore_payload(track_id, region_id, before),
            state_payload(
                track_id,
                right_id,
                HIRARI_REGION_EDIT_REMOVE,
                HirariRegionState::default(),
            ),
        ];
        let redo = [
            restore_payload(track_id, region_id, left_snapshot),
            restore_payload(track_id, right_id, right_snapshot),
        ];
        record_with(ops, host, b"Split Region\0", &undo, &redo, true, false);
    } else {
        unsafe {
            release_snapshot(host, before);
            release_snapshot(host, left_snapshot);
            release_snapshot(host, right_snapshot);
        }
    }
    right_id
}

/// Duplicates one region at `start_sample`.
///
/// The native copier kept every payload field, kept one identity across
/// undo/redo and recorded the add/remove pair in C++. Rust owns that decision
/// and the undo capture; the host clones the payload.
fn duplicate_region_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    start_sample: u64,
) -> u32 {
    let (Some(find), Some(clone), Some(snapshot)) =
        (ops.find_region, ops.clone_region, ops.snapshot_region)
    else {
        return 0;
    };
    let mut state = HirariRegionState::default();
    if !unsafe { find(host, track_id, region_id, &mut state) } {
        return 0;
    }
    let mut duplicate = HirariRegionState::default();
    if !unsafe { hirari_region_duplicate_state(&state, start_sample, &mut duplicate) } {
        return 0;
    }
    let duplicate_id = unsafe { clone(host, track_id, region_id, &duplicate) };
    if duplicate_id == 0 || ops.record_undo.is_none() {
        return duplicate_id;
    }
    // A duplicated clip is a new timeline instance that keeps one stable
    // identity across undo/redo, so the undo entry is built from a snapshot
    // instead of re-cloning on every redo.
    let snapshot_handle = unsafe { snapshot(host, track_id, duplicate_id) };
    if snapshot_handle.is_null() {
        return duplicate_id;
    }
    let undo = [state_payload(
        track_id,
        duplicate_id,
        HIRARI_REGION_EDIT_REMOVE,
        HirariRegionState::default(),
    )];
    let redo = [restore_payload(track_id, duplicate_id, snapshot_handle)];
    // The native copier recorded through `performAction`, which always
    // coalesces: repeated duplicates in one gesture stay a single entry.
    record_with(
        ops,
        host,
        b"Duplicate Region\0",
        &undo,
        &redo,
        true,
        false,
    );
    duplicate_id
}

/// Splits a region at an absolute timeline sample position.
///
/// # Safety
/// `host` must be the opaque handle the installed host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_split(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    split_beat: f64,
) -> u32 {
    let Some(ops) = host_ops() else { return 0 };
    split_region_with(&ops, host, track_id, region_id, split_beat)
}

/// Duplicates a region at `start_sample`.
///
/// # Safety
/// `host` must be the opaque handle the installed host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_duplicate(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    start_sample: u64,
) -> u32 {
    let Some(ops) = host_ops() else { return 0 };
    duplicate_region_with(&ops, host, track_id, region_id, start_sample)
}

/// Installs the native host callbacks. Passing null clears them.
#[no_mangle]
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
struct NoteEditContext {
    inner: *mut c_void,
    mutate: HirariRegionNoteEditFn,
    before: *mut HirariRegionVariAudioStore,
    after: *mut HirariRegionVariAudioStore,
}

unsafe extern "C" fn note_edit_trampoline(
    context: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    let context = &mut *(context as *mut NoteEditContext);
    if store.is_null() {
        return false;
    }
    context.before = vari_store_clone(store);
    if !(context.mutate)(context.inner, store) {
        return false;
    }
    context.after = vari_store_clone(store);
    true
}

fn note_store_payload(
    track_id: u32,
    region_id: u32,
    snapshot: *mut HirariRegionVariAudioStore,
) -> HirariRegionEditPayload {
    HirariRegionEditPayload {
        track_id,
        region_id,
        op: HIRARI_REGION_EDIT_NOTE_STORE,
        blob: snapshot as *const u8,
        ..Default::default()
    }
}

fn note_action_name(prefix: &str, track_id: u32, region_id: u32) -> Vec<u8> {
    let mut name = format!("{prefix} {track_id}:{region_id}").into_bytes();
    name.push(0);
    name
}

/// Runs `mutate` under the host's region lock, then records the before/after
/// tables as one undo entry. The host takes ownership of both snapshots.
fn note_edit_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    name: &[u8],
    inner: *mut c_void,
    mutate: HirariRegionNoteEditFn,
    coalesce: bool,
) -> bool {
    let (Some(note_edit), Some(find)) = (ops.note_edit, ops.find_region) else {
        return false;
    };
    let mut state = HirariRegionState::default();
    if !unsafe { find(host, track_id, region_id, &mut state) } {
        return false;
    }
    let mut context = NoteEditContext {
        inner,
        mutate,
        before: std::ptr::null_mut(),
        after: std::ptr::null_mut(),
    };
    let applied = unsafe {
        note_edit(
            host,
            track_id,
            region_id,
            &mut context as *mut NoteEditContext as *mut c_void,
            note_edit_trampoline,
        )
    };
    if !applied || ops.record_undo.is_none() {
        unsafe {
            vari_store_release(context.before);
            vari_store_release(context.after);
        }
        return applied;
    }
    let undo = note_store_payload(track_id, region_id, context.before);
    let redo = note_store_payload(track_id, region_id, context.after);
    record(ops, host, name, &[undo], &[redo], coalesce);
    true
}

#[repr(C)]
struct AnchorContext {
    segment_start: f64,
    position: f64,
    pitch_cents: f64,
    formant_cents: f64,
}

unsafe extern "C" fn mutate_anchor(
    context: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    let context = &*(context as *const AnchorContext);
    hirari_region_vari_audio_store_upsert_anchor(
        store,
        context.segment_start,
        context.position,
        context.pitch_cents,
        context.formant_cents,
    )
}

#[repr(C)]
struct ComponentContext {
    segment_start: f64,
    position: f64,
    cents: f64,
    formant: bool,
}

unsafe extern "C" fn mutate_component_anchor(
    context: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    let context = &*(context as *const ComponentContext);
    let Some(view) = vari_store_view(store, context.segment_start) else {
        return false;
    };
    if context.position < view.start || context.position > view.end {
        return false;
    }
    let mut anchor = HirariAudioNoteAnchor::default();
    if !hirari_audio_note_curve_component_anchor(
        view.anchors.as_ptr(),
        view.anchors.len(),
        view.start,
        view.end,
        context.position,
        context.cents,
        if context.formant { 1 } else { 0 },
        view.pitch_offset,
        view.formant_offset,
        &mut anchor,
    ) {
        return false;
    }
    hirari_region_vari_audio_store_upsert_anchor(
        store,
        context.segment_start,
        anchor.position_seconds,
        anchor.pitch_cents,
        anchor.formant_cents,
    )
}

#[repr(C)]
struct RemoveContext {
    segment_start: f64,
}

unsafe extern "C" fn mutate_remove(
    context: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    let context = &*(context as *const RemoveContext);
    hirari_region_vari_audio_store_remove(store, context.segment_start)
}

unsafe extern "C" fn mutate_clear(
    _context: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    hirari_region_vari_audio_store_clear_segments(store)
}

#[repr(C)]
struct WarpContext {
    segment_start: f64,
    new_start: f64,
    new_end: f64,
}

unsafe extern "C" fn mutate_warp(
    context: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    let context = &*(context as *const WarpContext);
    hirari_region_vari_audio_store_warp(
        store,
        context.segment_start,
        context.new_start,
        context.new_end,
    )
}

fn note_anchor_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    position: f64,
    pitch_cents: f64,
    formant_cents: f64,
) -> bool {
    if !segment_start.is_finite()
        || segment_start < 0.0
        || !position.is_finite()
        || position < 0.0
        || !pitch_cents.is_finite()
        || pitch_cents.abs() > 4800.0
        || !formant_cents.is_finite()
        || formant_cents.abs() > 2400.0
    {
        return false;
    }
    let name = note_action_name("Edit Audio Pitch Curve", track_id, region_id);
    let mut context = AnchorContext {
        segment_start,
        position,
        pitch_cents,
        formant_cents,
    };
    note_edit_with(
        ops,
        host,
        track_id,
        region_id,
        &name,
        &mut context as *mut AnchorContext as *mut c_void,
        mutate_anchor,
        true,
    )
}

fn note_pitch_anchor_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    position: f64,
    pitch_cents: f64,
) -> bool {
    if !segment_start.is_finite()
        || segment_start < 0.0
        || !position.is_finite()
        || position < 0.0
        || !pitch_cents.is_finite()
        || pitch_cents.abs() > 4800.0
    {
        return false;
    }
    let name = note_action_name("Edit Audio Pitch Curve", track_id, region_id);
    let mut context = ComponentContext {
        segment_start,
        position,
        cents: pitch_cents,
        formant: false,
    };
    note_edit_with(
        ops,
        host,
        track_id,
        region_id,
        &name,
        &mut context as *mut ComponentContext as *mut c_void,
        mutate_component_anchor,
        true,
    )
}

fn note_formant_anchor_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    position: f64,
    formant_cents: f64,
) -> bool {
    if !segment_start.is_finite()
        || segment_start < 0.0
        || !position.is_finite()
        || position < 0.0
        || !formant_cents.is_finite()
        || formant_cents.abs() > 2400.0
    {
        return false;
    }
    let name = note_action_name("Edit Audio Formant Curve", track_id, region_id);
    let mut context = ComponentContext {
        segment_start,
        position,
        cents: formant_cents,
        formant: true,
    };
    note_edit_with(
        ops,
        host,
        track_id,
        region_id,
        &name,
        &mut context as *mut ComponentContext as *mut c_void,
        mutate_component_anchor,
        true,
    )
}

fn note_remove_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
) -> bool {
    if !segment_start.is_finite() || segment_start < 0.0 {
        return false;
    }
    let mut context = RemoveContext { segment_start };
    note_edit_with(
        ops,
        host,
        track_id,
        region_id,
        b"Remove Audio Pitch Segment\0",
        &mut context as *mut RemoveContext as *mut c_void,
        mutate_remove,
        false,
    )
}

fn note_clear_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> bool {
    note_edit_with(
        ops,
        host,
        track_id,
        region_id,
        b"Clear Audio Pitch Segments\0",
        std::ptr::null_mut(),
        mutate_clear,
        false,
    )
}

fn note_warp_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    new_start: f64,
    new_end: f64,
) -> bool {
    if !segment_start.is_finite()
        || segment_start < 0.0
        || !new_start.is_finite()
        || new_start < 0.0
        || !new_end.is_finite()
        || new_end <= new_start
        || new_end - new_start > 24.0 * 60.0
    {
        return false;
    }
    let mut context = WarpContext {
        segment_start,
        new_start,
        new_end,
    };
    note_edit_with(
        ops,
        host,
        track_id,
        region_id,
        b"Warp Audio Pitch Segment\0",
        &mut context as *mut WarpContext as *mut c_void,
        mutate_warp,
        false,
    )
}

#[repr(C)]
struct SegmentContext {
    start: f64,
    end: f64,
    pitch_cents: f64,
    formant_cents: f64,
}

unsafe extern "C" fn mutate_segment(
    context: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    let context = &*(context as *const SegmentContext);
    // Re-editing the same span keeps its detected pitch and curve anchors, the
    // way the C++ version reused the existing segment before overwriting the
    // offsets.
    let existing = vari_store_view(store, context.start)
        .filter(|view| (view.end - context.end).abs() < 1e-9);
    let (detected_pitch, anchors) = match existing {
        Some(view) => (view.detected_pitch, view.anchors),
        None => (0.0, Vec::new()),
    };
    let segment = HirariAudioNoteSegment {
        start_seconds: context.start,
        end_seconds: context.end,
        detected_pitch_cents: detected_pitch,
        pitch_offset_cents: context.pitch_cents.clamp(-4800.0, 4800.0),
        formant_offset_cents: context.formant_cents.clamp(-2400.0, 2400.0),
    };
    hirari_region_vari_audio_store_upsert(store, segment, anchors.as_ptr(), anchors.len())
}

#[repr(C)]
struct MoveAnchorContext {
    segment_start: f64,
    old_position: f64,
    new_position: f64,
    value_cents: f64,
    formant: bool,
}

unsafe extern "C" fn mutate_move_anchor(
    context: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    let context = &*(context as *const MoveAnchorContext);
    let Some(view) = vari_store_view(store, context.segment_start) else {
        return false;
    };
    if context.new_position < view.start || context.new_position > view.end {
        return false;
    }
    let mut anchors = view.anchors;
    let moved = hirari_audio_note_curve_move_anchor(
        anchors.as_mut_ptr(),
        anchors.len(),
        view.start,
        view.end,
        context.old_position,
        context.new_position,
        context.value_cents,
        if context.formant { 1 } else { 0 },
        view.pitch_offset,
        view.formant_offset,
    );
    if moved > anchors.len() {
        return false;
    }
    anchors.truncate(moved);
    let segment = HirariAudioNoteSegment {
        start_seconds: view.start,
        end_seconds: view.end,
        detected_pitch_cents: view.detected_pitch,
        pitch_offset_cents: view.pitch_offset,
        formant_offset_cents: view.formant_offset,
    };
    hirari_region_vari_audio_store_upsert(store, segment, anchors.as_ptr(), anchors.len())
}

fn note_segment_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    start: f64,
    end: f64,
    pitch_cents: f64,
    formant_cents: f64,
) -> bool {
    if !start.is_finite()
        || !end.is_finite()
        || end <= start
        || end - start > 24.0 * 60.0
        || !pitch_cents.is_finite()
        || !formant_cents.is_finite()
        || pitch_cents.abs() > 4800.0
        || formant_cents.abs() > 2400.0
    {
        return false;
    }
    let name = note_action_name("Edit Audio Pitch Segments", track_id, region_id);
    let mut context = SegmentContext {
        start,
        end,
        pitch_cents,
        formant_cents,
    };
    note_edit_with(
        ops,
        host,
        track_id,
        region_id,
        &name,
        &mut context as *mut SegmentContext as *mut c_void,
        mutate_segment,
        true,
    )
}

fn note_move_anchor_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    old_position: f64,
    new_position: f64,
    value_cents: f64,
    formant: bool,
) -> bool {
    let max_cents = if formant { 2400.0 } else { 4800.0 };
    if !segment_start.is_finite()
        || segment_start < 0.0
        || !old_position.is_finite()
        || old_position < 0.0
        || !new_position.is_finite()
        || new_position < 0.0
        || !value_cents.is_finite()
        || value_cents.abs() > max_cents
    {
        return false;
    }
    let prefix = if formant {
        "Move Audio Formant Anchor"
    } else {
        "Move Audio Pitch Anchor"
    };
    let name = note_action_name(prefix, track_id, region_id);
    let mut context = MoveAnchorContext {
        segment_start,
        old_position,
        new_position,
        value_cents,
        formant,
    };
    note_edit_with(
        ops,
        host,
        track_id,
        region_id,
        &name,
        &mut context as *mut MoveAnchorContext as *mut c_void,
        mutate_move_anchor,
        true,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_note_segment(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    start: f64,
    end: f64,
    pitch_cents: f64,
    formant_cents: f64,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    note_segment_with(
        &ops,
        host,
        track_id,
        region_id,
        start,
        end,
        pitch_cents,
        formant_cents,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_note_move_anchor(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    old_position: f64,
    new_position: f64,
    value_cents: f64,
    formant: bool,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    note_move_anchor_with(
        &ops,
        host,
        track_id,
        region_id,
        segment_start,
        old_position,
        new_position,
        value_cents,
        formant,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_note_anchor(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    position: f64,
    pitch_cents: f64,
    formant_cents: f64,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    note_anchor_with(
        &ops,
        host,
        track_id,
        region_id,
        segment_start,
        position,
        pitch_cents,
        formant_cents,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_note_pitch_anchor(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    position: f64,
    pitch_cents: f64,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    note_pitch_anchor_with(
        &ops,
        host,
        track_id,
        region_id,
        segment_start,
        position,
        pitch_cents,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_note_formant_anchor(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    position: f64,
    formant_cents: f64,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    note_formant_anchor_with(
        &ops,
        host,
        track_id,
        region_id,
        segment_start,
        position,
        formant_cents,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_note_remove(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    note_remove_with(&ops, host, track_id, region_id, segment_start)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_note_clear(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    note_clear_with(&ops, host, track_id, region_id)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_note_warp(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    new_start: f64,
    new_end: f64,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    note_warp_with(
        &ops,
        host,
        track_id,
        region_id,
        segment_start,
        new_start,
        new_end,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_note_store_restore(
    snapshot: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    vari_store_restore(store, snapshot as *mut HirariRegionVariAudioStore)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::slice;

use crate::vari_audio::{
    hirari_region_vari_audio_store_clear_segments, hirari_region_vari_audio_store_new,
    hirari_region_vari_audio_store_remove, hirari_region_vari_audio_store_upsert,
    hirari_region_vari_audio_store_upsert_anchor, hirari_region_vari_audio_store_warp,
    vari_store_clone, vari_store_release, vari_store_restore, vari_store_view,
    HirariAudioNoteSegment, HirariRegionVariAudioStore,
};
use crate::audio_note_curve::{
    hirari_audio_note_curve_component_anchor, HirariAudioNoteAnchor,
};
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeHost {
        present: bool,
        state: HirariRegionState,
        applied: Vec<HirariRegionEditPayload>,
        applied_blobs: Vec<Vec<u8>>,
        undo: Vec<HirariRegionEditPayload>,
        redo: Vec<HirariRegionEditPayload>,
        undo_name: String,
        coalesce: bool,
        perform: bool,
        note_store: usize,
        warp_markers: Vec<HirariWarpMarker>,
        undo_markers: Vec<HirariWarpMarker>,
        redo_markers: Vec<HirariWarpMarker>,
        range_edits: Vec<NativeRangeEdit>,
        comp_ranges: Vec<NativeCompRange>,
        comp_managed: bool,
        cloned_states: Vec<HirariRegionState>,
        next_region_id: u32,
    }

    static FAKE: OnceLock<Mutex<FakeHost>> = OnceLock::new();
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn fake() -> &'static Mutex<FakeHost> {
        FAKE.get_or_init(|| Mutex::new(FakeHost::default()))
    }

    unsafe extern "C" fn fake_find(
        _host: *mut c_void,
        _track_id: u32,
        _region_id: u32,
        out: *mut HirariRegionState,
    ) -> bool {
        let guard = fake().lock().unwrap();
        if !guard.present {
            return false;
        }
        unsafe { out.write(guard.state) };
        true
    }

    unsafe extern "C" fn fake_apply(
        _host: *mut c_void,
        payload: *const HirariRegionEditPayload,
    ) -> bool {
        if FAIL_APPLY.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        let payload = unsafe { ptr::read(payload) };
        let blob = if payload.blob_len != 0 && !payload.blob.is_null() {
            unsafe { slice::from_raw_parts(payload.blob, payload.blob_len) }.to_vec()
        } else {
            Vec::new()
        };
        let mut guard = fake().lock().unwrap();
        guard.applied_blobs.push(blob);
        guard.applied.push(payload);
        true
    }

    static FAIL_APPLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static RELEASED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static SNAPSHOT_TARGET: u8 = 0;

    unsafe extern "C" fn fake_snapshot(
        _host: *mut c_void,
        _track_id: u32,
        _region_id: u32,
    ) -> *mut c_void {
        &SNAPSHOT_TARGET as *const u8 as *mut c_void
    }

    unsafe extern "C" fn fake_release(_host: *mut c_void, _handle: *mut c_void) {
        RELEASED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    unsafe extern "C" fn fake_list(
        _host: *mut c_void,
        _group: u32,
        out: *mut HirariRegionGroupMember,
        capacity: usize,
    ) -> usize {
        if out.is_null() || capacity < 2 {
            return 2;
        }
        let state = fake().lock().unwrap().state;
        unsafe {
            out.write(HirariRegionGroupMember {
                track_id: 1,
                region_id: 2,
                start: state.start,
                base_start: state.base_start,
                locked: false,
            });
            out.add(1).write(HirariRegionGroupMember {
                track_id: 1,
                region_id: 3,
                start: state.start + 100,
                base_start: state.base_start + 100,
                locked: false,
            });
        }
        2
    }

    unsafe extern "C" fn fake_note_edit(
        _host: *mut c_void,
        _track_id: u32,
        _region_id: u32,
        context: *mut c_void,
        mutate: HirariRegionNoteEditFn,
    ) -> bool {
        let store = {
            let guard = fake().lock().unwrap();
            if !guard.present {
                return false;
            }
            guard.note_store as *mut HirariRegionVariAudioStore
        };
        if store.is_null() {
            return false;
        }
        unsafe { mutate(context, store) }
    }

    unsafe extern "C" fn fake_record_undo(
        _host: *mut c_void,
        name: *const c_char,
        undo: *const HirariRegionEditPayload,
        undo_count: usize,
        redo: *const HirariRegionEditPayload,
        redo_count: usize,
        coalesce: bool,
        perform: bool,
    ) -> bool {
        let mut guard = fake().lock().unwrap();
        guard.coalesce = coalesce;
        guard.perform = perform;
        guard.undo_name = unsafe { std::ffi::CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned();
        let undo = unsafe { slice::from_raw_parts(undo, undo_count) }.to_vec();
        let redo = unsafe { slice::from_raw_parts(redo, redo_count) }.to_vec();
        let copy_markers = |payloads: &[HirariRegionEditPayload]| {
            payloads
                .iter()
                .find(|payload| {
                    payload.op == HIRARI_REGION_EDIT_GEOMETRY && !payload.blob.is_null()
                })
                .map(|payload| unsafe {
                    slice::from_raw_parts(
                        payload.blob.cast::<HirariWarpMarker>(),
                        payload.blob_len / std::mem::size_of::<HirariWarpMarker>(),
                    )
                    .to_vec()
                })
                .unwrap_or_default()
        };
        guard.undo_markers = copy_markers(&undo);
        guard.redo_markers = copy_markers(&redo);
        guard.undo = undo;
        guard.redo = redo;
        true
    }

    unsafe extern "C" fn fake_range_edits(
        _host: *mut c_void,
        _track_id: u32,
        _region_id: u32,
        out: *mut NativeRangeEdit,
        capacity: usize,
    ) -> usize {
        let guard = fake().lock().unwrap();
        let count = guard.range_edits.len();
        if out.is_null() || capacity < count {
            return count;
        }
        unsafe { ptr::copy_nonoverlapping(guard.range_edits.as_ptr(), out, count) };
        count
    }

    unsafe extern "C" fn fake_markers(
        _host: *mut c_void,
        _track_id: u32,
        _region_id: u32,
        out: *mut HirariWarpMarker,
        capacity: usize,
    ) -> usize {
        let guard = fake().lock().unwrap();
        let count = guard.warp_markers.len();
        if out.is_null() || capacity < count {
            return count;
        }
        unsafe { ptr::copy_nonoverlapping(guard.warp_markers.as_ptr(), out, count) };
        count
    }

    unsafe extern "C" fn fake_comp_ranges(
        _host: *mut c_void,
        _track_id: u32,
        _region_id: u32,
        out: *mut NativeCompRange,
        capacity: usize,
        managed: *mut bool,
    ) -> usize {
        let guard = fake().lock().unwrap();
        if !managed.is_null() {
            unsafe { *managed = guard.comp_managed };
        }
        let count = guard.comp_ranges.len();
        if out.is_null() || capacity < count {
            return count;
        }
        unsafe { ptr::copy_nonoverlapping(guard.comp_ranges.as_ptr(), out, count) };
        count
    }

    unsafe extern "C" fn fake_clone(
        _host: *mut c_void,
        _track_id: u32,
        _region_id: u32,
        state: *const HirariRegionState,
    ) -> u32 {
        let mut guard = fake().lock().unwrap();
        guard.cloned_states.push(if state.is_null() {
            HirariRegionState::default()
        } else {
            unsafe { *state }
        });
        guard.next_region_id = if guard.next_region_id == 0 {
            100
        } else {
            guard.next_region_id + 1
        };
        guard.next_region_id
    }

    unsafe extern "C" fn fake_retain_track(_host: *mut c_void, _track_id: u32) -> *mut c_void {
        1usize as *mut c_void
    }

    unsafe extern "C" fn fake_release_track(_handle: *mut c_void) {}

    unsafe extern "C" fn fake_note_snapshot(
        _handle: *mut c_void,
        _region_id: u32,
    ) -> *mut c_void {
        hirari_region_vari_audio_store_new().cast()
    }

    static FAKE_AUDIO: u8 = 0;
    static FAIL_DECODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static RELEASED_AUDIO: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    static FAKE_RATE_BITS: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(48_000f64.to_bits());

    unsafe extern "C" fn fake_decode_audio(
        _host: *mut c_void,
        _path: *const u8,
        _length: usize,
        out: *mut *mut c_void,
        sample_rate: *mut f64,
    ) -> bool {
        if FAIL_DECODE.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        unsafe {
            out.write(&FAKE_AUDIO as *const u8 as *mut c_void);
            sample_rate.write(44_100.0);
        }
        true
    }

    unsafe extern "C" fn fake_release_audio(_handle: *mut c_void) {
        RELEASED_AUDIO.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    unsafe extern "C" fn fake_sample_rate(_host: *mut c_void) -> f64 {
        f64::from_bits(FAKE_RATE_BITS.load(std::sync::atomic::Ordering::SeqCst))
    }

    fn fake_ops() -> HirariRegionHostOps {
        HirariRegionHostOps {
            find_region: Some(fake_find),
            apply_edit: Some(fake_apply),
            list_sync_group: Some(fake_list),
            record_undo: Some(fake_record_undo),
            note_edit: Some(fake_note_edit),
            snapshot_region: Some(fake_snapshot),
            release_snapshot: Some(fake_release),
            warp_markers: Some(fake_markers),
            range_edits: Some(fake_range_edits),
            sample_rate: Some(fake_sample_rate),
            decode_audio: Some(fake_decode_audio),
            release_audio: Some(fake_release_audio),
            clone_region: Some(fake_clone),
            comp_ranges: Some(fake_comp_ranges),
            retain_track: Some(fake_retain_track),
            release_track: Some(fake_release_track),
            note_snapshot: Some(fake_note_snapshot),
            ..Default::default()
        }
    }

    fn reset(state: HirariRegionState) {
        *fake().lock().unwrap() = FakeHost {
            present: true,
            state,
            ..Default::default()
        };
    }

    #[test]
    fn scalar_edit_clamps_and_records_one_undo_entry() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            clip_gain: 1.0,
            len: 100,
            start: 0,
            ..Default::default()
        });

        assert!(edit_scalar_with(&ops, host, 1, 2, 1, 2.5, 0.0));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_INPLACE);
        assert!((guard.applied[0].state.clip_gain - 2.0).abs() < 1e-6);
        assert_eq!(guard.undo_name, "Region Gain");
        assert_eq!(guard.undo.len(), 1);
        assert_eq!(guard.redo.len(), 1);
        assert!((guard.undo[0].state.clip_gain - 1.0).abs() < 1e-6);
        assert!((guard.redo[0].state.clip_gain - 2.0).abs() < 1e-6);
        drop(guard);

    }

    #[test]
    fn scalar_edit_rejects_non_finite_without_applying() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            ..Default::default()
        });

        assert!(!edit_scalar_with(&ops, host, 1, 2, 1, f32::NAN, 0.0));
        assert!(fake().lock().unwrap().applied.is_empty());

    }

    fn trim_base_state() -> HirariRegionState {
        HirariRegionState {
            start: 1000,
            len: 1000,
            base_start: 1000,
            base_length: 2000,
            source_offset: 0,
            base_source_offset: 0,
            source_length: 0,
            loop_count: 1,
            fade_in_samples: 64,
            fade_out_samples: 64,
            warp_ratio: 1.0,
            ..Default::default()
        }
    }

    fn one_marker() -> HirariWarpMarker {
        HirariWarpMarker {
            source_sample: 10,
            timeline_sample: 12,
            transient: 0,
        }
    }

    #[test]
    fn trim_edit_applies_the_geometry_and_records_the_markers_for_undo() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(trim_base_state());
        fake().lock().unwrap().warp_markers = vec![one_marker()];

        assert!(edit_trim_with(&ops, host, 1, 2, 0.25, 0.75));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_GEOMETRY);
        assert_eq!(guard.applied[0].state.start, 1500);
        assert_eq!(guard.applied[0].state.len, 1000);
        assert_eq!(guard.applied[0].state.source_offset, 500);
        assert_eq!(guard.applied[0].state.source_length, 1000);
        assert!(guard.applied[0].blob.is_null());
        assert_eq!(guard.undo_name, "Trim Region");
        assert!(!guard.coalesce);
        assert_eq!(guard.undo[0].state.start, 1000);
        assert_eq!(guard.undo[0].state.source_offset, 0);
        assert_eq!(guard.undo_markers.len(), 1);
        assert_eq!(guard.undo_markers[0].source_sample, 10);
        assert_eq!(guard.undo_markers[0].timeline_sample, 12);
        assert!(guard.redo_markers.is_empty());
    }

    #[test]
    fn trim_edit_clamps_the_fractions_and_rejects_inverted_or_locked_regions() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(trim_base_state());

        assert!(!edit_trim_with(&ops, host, 1, 2, 0.8, 0.2));
        assert!(!edit_trim_with(&ops, host, 1, 2, f32::NAN, 0.5));
        assert!(fake().lock().unwrap().applied.is_empty());

        let mut locked = trim_base_state();
        locked.locked = true;
        reset(locked);
        assert!(!edit_trim_with(&ops, host, 1, 2, 0.25, 0.75));
        assert!(fake().lock().unwrap().applied.is_empty());

        // Out-of-range fractions clamp instead of rejecting, like the C++
        // wrapper did before handing them to the geometry transform.
        reset(trim_base_state());
        assert!(edit_trim_with(&ops, host, 1, 2, -0.5, 1.5));
        let guard = fake().lock().unwrap();
        assert_eq!(guard.undo[0].state.start, 1000);
    }

    #[test]
    fn warp_ratio_edit_recomputes_the_length_and_clears_markers_on_redo() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        let state = HirariRegionState {
            len: 1000,
            base_length: 1000,
            source_length: 1000,
            loop_count: 1,
            warp_ratio: 1.0,
            ..Default::default()
        };
        reset(state);
        fake().lock().unwrap().warp_markers = vec![one_marker()];

        assert!(edit_warp_ratio_with(&ops, host, 1, 2, 2.0));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_GEOMETRY);
        assert_eq!(guard.applied[0].state.len, 500);
        assert!((guard.applied[0].state.warp_ratio - 2.0).abs() < 1e-9);
        assert_eq!(guard.undo_name, "Flex Warp Ratio");
        assert_eq!(guard.undo[0].state.len, 1000);
        assert!((guard.undo[0].state.warp_ratio - 1.0).abs() < 1e-9);
        assert_eq!(guard.undo_markers.len(), 1);
        assert_eq!(guard.undo_markers[0].source_sample, 10);
        assert_eq!(guard.undo_markers[0].timeline_sample, 12);
        assert!(guard.redo_markers.is_empty());
    }

    #[test]
    fn warp_ratio_edit_rejects_out_of_range_values() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(trim_base_state());

        assert!(!edit_warp_ratio_with(&ops, host, 1, 2, 0.49));
        assert!(!edit_warp_ratio_with(&ops, host, 1, 2, 2.01));
        assert!(!edit_warp_ratio_with(&ops, host, 1, 2, f64::NAN));
        assert!(fake().lock().unwrap().applied.is_empty());
    }

    #[test]
    fn remove_applies_then_records_a_restore_that_owns_the_snapshot() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState { start: 10, len: 100, ..Default::default() });
        RELEASED.store(0, std::sync::atomic::Ordering::SeqCst);

        assert!(edit_remove_with(&ops, host, 1, 2));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_REMOVE);
        assert_eq!(guard.undo_name, "Remove Region");
        assert!(!guard.coalesce);
        assert_eq!(guard.undo.len(), 1);
        assert_eq!(guard.undo[0].op, HIRARI_REGION_EDIT_RESTORE);
        assert_eq!(guard.undo[0].blob, &SNAPSHOT_TARGET as *const u8);
        assert_eq!(guard.redo.len(), 1);
        assert_eq!(guard.redo[0].op, HIRARI_REGION_EDIT_REMOVE);
        assert!(guard.redo[0].blob.is_null());
        // Ownership moved to the undo entry, so nothing was released here.
        assert_eq!(RELEASED.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn failed_remove_releases_the_snapshot_and_records_nothing() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState { len: 100, ..Default::default() });
        RELEASED.store(0, std::sync::atomic::Ordering::SeqCst);
        FAIL_APPLY.store(true, std::sync::atomic::Ordering::SeqCst);

        let removed = edit_remove_with(&ops, host, 1, 2);
        FAIL_APPLY.store(false, std::sync::atomic::Ordering::SeqCst);

        assert!(!removed);
        assert_eq!(RELEASED.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(fake().lock().unwrap().undo.is_empty());
    }

    #[test]
    fn remove_rejects_zero_ids_and_missing_regions() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState { len: 100, ..Default::default() });
        assert!(!edit_remove_with(&ops, host, 0, 2));
        assert!(!edit_remove_with(&ops, host, 1, 0));
        fake().lock().unwrap().present = false;
        assert!(!edit_remove_with(&ops, host, 1, 2));
        assert!(fake().lock().unwrap().applied.is_empty());
    }

    #[test]
    fn missing_region_is_reported() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState::default());
        fake().lock().unwrap().present = false;

        assert!(!edit_scalar_with(&ops, host, 1, 2, 1, 0.5, 0.0));

    }

    #[test]
    fn move_keeps_the_untrimmed_base_start_equal_to_start() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            start: 10,
            len: 100,
            base_start: 10,
            source_offset: 5,
            base_source_offset: 5,
            warp_ratio: 1.0,
            ..Default::default()
        });

        assert!(edit_move_with(&ops, host, 1, 2, 4096.0));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_STATE);
        assert_eq!(guard.applied[0].state.start, 4096);
        assert_eq!(guard.applied[0].state.base_start, 4096);
        assert_eq!(guard.undo[0].state.start, 10);
        assert_eq!(guard.undo_name, "Move Region");
        drop(guard);

    }

    #[test]
    fn sync_group_move_applies_every_member() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            start: 100,
            len: 50,
            base_start: 100,
            sync_group: 7,
            warp_ratio: 1.0,
            ..Default::default()
        });

        assert!(edit_move_sync_group_with(&ops, host, 1, 2, 400.0));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 2);
        assert_eq!(guard.undo.len(), 2);
        assert_eq!(guard.redo.len(), 2);
        assert_eq!(guard.applied[0].state.start, 400);
        assert_eq!(guard.applied[1].state.start, 500);
        assert_eq!(guard.undo_name, "Move Region Sync Group");
        drop(guard);

    }

    #[test]
    fn locking_a_region_ignores_the_lock_and_records_no_undo() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            locked: true,
            ..Default::default()
        });

        assert!(edit_scalar_with(&ops, host, 1, 2, 8, 1.0, 0.0));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert!(guard.applied[0].state.locked);
        assert!(guard.undo.is_empty());
        assert!(guard.redo.is_empty());
        drop(guard);
    }

    #[test]
    fn pitch_preserve_backfills_the_source_length_before_recording() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            source_length: 0,
            ..Default::default()
        });

        assert!(edit_scalar_with(&ops, host, 1, 2, 9, 1.0, 0.0));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert!(guard.applied[0].state.pitch_preserve_warp);
        assert_eq!(guard.applied[0].state.source_length, 100);
        assert_eq!(guard.undo_name, "Preserve Warp Pitch");
        drop(guard);
    }

    #[test]
    fn note_anchor_edit_records_the_whole_table_before_and_after() {
        use crate::vari_audio::{
            hirari_region_vari_audio_store_new, hirari_region_vari_audio_store_upsert,
            HirariAudioNoteSegment,
        };

        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 44_100,
            ..Default::default()
        });
        let store = unsafe { hirari_region_vari_audio_store_new() };
        let segment = HirariAudioNoteSegment {
            start_seconds: 0.0,
            end_seconds: 1.0,
            detected_pitch_cents: 0.0,
            pitch_offset_cents: 0.0,
            formant_offset_cents: 0.0,
        };
        assert!(unsafe {
            hirari_region_vari_audio_store_upsert(store, segment, ptr::null(), 0)
        });
        fake().lock().unwrap().note_store = store as usize;
        assert!(note_anchor_with(&ops, host, 1, 2, 0.0, 0.5, 100.0, 0.0));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.undo.len(), 1);
        assert_eq!(guard.redo.len(), 1);
        assert_eq!(guard.undo[0].op, HIRARI_REGION_EDIT_NOTE_STORE);
        assert_eq!(guard.redo[0].op, HIRARI_REGION_EDIT_NOTE_STORE);
        assert!(!guard.undo[0].blob.is_null());
        assert!(!guard.redo[0].blob.is_null());
        assert_eq!(guard.undo_name, "Edit Audio Pitch Curve 1:2");
        let before = guard.undo[0].blob as *mut HirariRegionVariAudioStore;
        let after = guard.redo[0].blob as *mut HirariRegionVariAudioStore;
        drop(guard);

        unsafe {
            crate::vari_audio::vari_store_release(before);
            crate::vari_audio::vari_store_release(after);
            crate::vari_audio::vari_store_release(store);
        }
        fake().lock().unwrap().note_store = 0;
    }

    #[test]
    fn note_anchor_edit_rejects_out_of_range_input_before_touching_the_host() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState::default());

        assert!(!note_anchor_with(&ops, host, 1, 2, 0.0, 0.5, 5000.0, 0.0));
        assert!(!note_warp_with(&ops, host, 1, 2, 0.0, 1.0, 1.0));
        assert!(fake().lock().unwrap().undo.is_empty());
    }

    fn store_with_segment(detected: f64, anchor_position: f64) -> *mut HirariRegionVariAudioStore {
        use crate::vari_audio::{
            hirari_region_vari_audio_store_new, hirari_region_vari_audio_store_upsert,
            HirariAudioNoteSegment,
        };

        let store = unsafe { hirari_region_vari_audio_store_new() };
        let segment = HirariAudioNoteSegment {
            start_seconds: 0.0,
            end_seconds: 1.0,
            detected_pitch_cents: detected,
            pitch_offset_cents: 0.0,
            formant_offset_cents: 0.0,
        };
        let anchor = HirariAudioNoteAnchor {
            position_seconds: anchor_position,
            pitch_cents: 50.0,
            formant_cents: 0.0,
        };
        assert!(unsafe { hirari_region_vari_audio_store_upsert(store, segment, &anchor, 1) });
        store
    }

    #[test]
    fn note_segment_edit_keeps_the_detected_pitch_and_curve() {
        use crate::vari_audio::{hirari_region_vari_audio_store_get, vari_store_release};

        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState::default());
        let store = store_with_segment(42.0, 0.5);
        fake().lock().unwrap().note_store = store as usize;

        assert!(note_segment_with(&ops, host, 1, 2, 0.0, 1.0, 100.0, -30.0));

        let mut stored = crate::vari_audio::HirariAudioNoteSegment::default();
        assert!(unsafe { hirari_region_vari_audio_store_get(store, 0, &mut stored) });
        assert_eq!(stored.detected_pitch_cents, 42.0);
        assert_eq!(stored.pitch_offset_cents, 100.0);
        assert_eq!(stored.formant_offset_cents, -30.0);
        assert_eq!(unsafe { crate::vari_audio::vari_store_view(store, 0.0) }.unwrap().anchors.len(), 1);

        let guard = fake().lock().unwrap();
        assert_eq!(guard.undo_name, "Edit Audio Pitch Segments 1:2");
        assert!(guard.coalesce);
        let undo = guard.undo[0].blob as *mut HirariRegionVariAudioStore;
        let redo = guard.redo[0].blob as *mut HirariRegionVariAudioStore;
        drop(guard);
        unsafe {
            vari_store_release(undo);
            vari_store_release(redo);
            vari_store_release(store);
        }
        fake().lock().unwrap().note_store = 0;
    }

    #[test]
    fn note_move_anchor_repositions_the_anchor() {
        use crate::vari_audio::vari_store_release;

        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState::default());
        let store = store_with_segment(0.0, 0.5);
        fake().lock().unwrap().note_store = store as usize;

        assert!(note_move_anchor_with(&ops, host, 1, 2, 0.0, 0.5, 0.8, 70.0, false));
        let view = unsafe { crate::vari_audio::vari_store_view(store, 0.0) }.unwrap();
        assert_eq!(view.anchors.len(), 1);
        assert!((view.anchors[0].position_seconds - 0.8).abs() < 1e-9);
        assert!((view.anchors[0].pitch_cents - 70.0).abs() < 1e-9);

        let guard = fake().lock().unwrap();
        assert_eq!(guard.undo_name, "Move Audio Pitch Anchor 1:2");
        let undo = guard.undo[0].blob as *mut HirariRegionVariAudioStore;
        let redo = guard.redo[0].blob as *mut HirariRegionVariAudioStore;
        drop(guard);
        unsafe {
            vari_store_release(undo);
            vari_store_release(redo);
            vari_store_release(store);
        }
        fake().lock().unwrap().note_store = 0;
    }

    fn marker(source: u64, timeline: u64, transient: u8) -> HirariWarpMarker {
        HirariWarpMarker {
            source_sample: source,
            timeline_sample: timeline,
            transient,
        }
    }

    #[test]
    fn warp_marker_edit_orders_markers_and_records_marker_undo() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 1_000,
            source_length: 2_000,
            ..Default::default()
        });
        fake().lock().unwrap().warp_markers = vec![marker(0, 0, 0)];

        let input = [marker(500, 500, 0), marker(100, 100, 1)];
        assert!(set_warp_markers_with(
            &ops,
            host,
            1,
            2,
            input.as_ptr(),
            input.len()
        ));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_GEOMETRY);
        assert_eq!(guard.undo_name, "Edit Region Warp Markers");
        assert!(!guard.coalesce);
        assert!(same_markers(&guard.undo_markers, &[marker(0, 0, 0)]));
        assert!(same_markers(
            &guard.redo_markers,
            &[marker(100, 100, 1), marker(500, 500, 0)]
        ));
    }

    #[test]
    fn warp_marker_edit_rejects_dropped_markers_and_locked_regions() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 1_000,
            source_length: 2_000,
            ..Default::default()
        });

        // A duplicate timeline anchor is dropped by normalization, so the
        // edit is rejected instead of silently shortening the list.
        let duplicate = [marker(0, 0, 0), marker(0, 0, 0)];
        assert!(!set_warp_markers_with(
            &ops,
            host,
            1,
            2,
            duplicate.as_ptr(),
            duplicate.len()
        ));
        // Beyond the region's timeline length.
        let beyond = [marker(0, 0, 0), marker(1_100, 1_100, 0)];
        assert!(!set_warp_markers_with(
            &ops,
            host,
            1,
            2,
            beyond.as_ptr(),
            beyond.len()
        ));
        // A null buffer with a non-zero count must not be read.
        assert!(!set_warp_markers_with(&ops, host, 1, 2, ptr::null(), 1));
        assert!(fake().lock().unwrap().applied.is_empty());

        fake().lock().unwrap().state.locked = true;
        let valid = [marker(0, 0, 0)];
        assert!(!set_warp_markers_with(
            &ops,
            host,
            1,
            2,
            valid.as_ptr(),
            valid.len()
        ));
    }

    #[test]
    fn warp_marker_edit_accepts_an_empty_list_as_a_clear() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 1_000,
            ..Default::default()
        });
        fake().lock().unwrap().warp_markers = vec![marker(0, 0, 0), marker(10, 10, 0)];

        assert!(set_warp_markers_with(&ops, host, 1, 2, ptr::null(), 0));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_GEOMETRY);
        assert!(guard.applied[0].blob.is_null());
        assert_eq!(guard.applied[0].blob_len, 0);
        assert_eq!(guard.undo_markers.len(), 2);
        assert!(guard.redo_markers.is_empty());
    }

    #[test]
    fn packed_warp_markers_convert_the_parallel_slices() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 1_000,
            source_length: 2_000,
            ..Default::default()
        });
        fake().lock().unwrap().warp_markers = vec![marker(0, 0, 0)];

        let source = [500u64, 100];
        let timeline = [500u64, 100];
        let flags = [0u8, 1];
        assert!(set_warp_markers_from_slices(
            &ops,
            host,
            1,
            2,
            source.as_ptr(),
            source.len(),
            timeline.as_ptr(),
            timeline.len(),
            flags.as_ptr(),
            flags.len(),
        ));

        let guard = fake().lock().unwrap();
        assert!(same_markers(
            &guard.redo_markers,
            &[marker(100, 100, 1), marker(500, 500, 0)]
        ));
    }

    #[test]
    fn packed_warp_markers_reject_disagreeing_lengths_and_oversized_lists() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState::default());
        let source = [1u64, 2];
        let timeline = [1u64];
        let flags = [1u8, 1];
        assert!(!set_warp_markers_from_slices(
            &ops,
            host,
            1,
            2,
            source.as_ptr(),
            source.len(),
            timeline.as_ptr(),
            timeline.len(),
            flags.as_ptr(),
            flags.len(),
        ));
        // The cap is enforced before any allocation, so null slices with an
        // oversized count are rejected without touching memory.
        assert!(!set_warp_markers_from_slices(
            &ops,
            host,
            1,
            2,
            ptr::null(),
            100_001,
            ptr::null(),
            100_001,
            ptr::null(),
            100_001,
        ));
    }

    #[test]
    fn range_edit_upsert_stores_the_list_and_performs_the_undo_pair() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            ..Default::default()
        });

        assert!(set_range_edit_with(&ops, host, 1, 2, 10, 50, 0.5, 2, 2));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_RANGE_EDITS);
        assert_eq!(guard.applied[0].blob_len, std::mem::size_of::<NativeRangeEdit>());
        assert_eq!(guard.undo_name, "Region Range Edit");
        assert!(guard.perform, "range edits keep the native perform path");
        assert!(guard.coalesce);
        assert_eq!(guard.undo[0].blob_len, 0);
        assert_eq!(guard.redo[0].blob_len, std::mem::size_of::<NativeRangeEdit>());
    }

    #[test]
    fn range_edit_rejects_out_of_bounds_and_locked_regions() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            ..Default::default()
        });

        assert!(!set_range_edit_with(&ops, host, 1, 2, 10, 101, 0.5, 2, 2));
        assert!(!set_range_edit_with(&ops, host, 1, 2, 10, 50, f32::NAN, 2, 2));
        fake().lock().unwrap().state.locked = true;
        assert!(!set_range_edit_with(&ops, host, 1, 2, 10, 50, 0.5, 2, 2));
        assert!(fake().lock().unwrap().applied.is_empty());
    }

    #[test]
    fn clearing_range_edits_records_the_previous_list() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            ..Default::default()
        });
        fake().lock().unwrap().range_edits = vec![NativeRangeEdit {
            start: 10,
            end: 50,
            gain: 0.5,
            fade_in: 2,
            fade_out: 2,
        }];

        assert!(clear_range_edits_with(&ops, host, 1, 2));

        {
            let guard = fake().lock().unwrap();
            assert_eq!(guard.undo_name, "Clear Region Range Edits");
            assert_eq!(guard.undo[0].blob_len, std::mem::size_of::<NativeRangeEdit>());
            assert_eq!(guard.redo[0].blob_len, 0);
        }
        fake().lock().unwrap().range_edits.clear();
        assert!(!clear_range_edits_with(&ops, host, 1, 2));
    }

    #[test]
    fn comp_range_batch_is_admitted_and_stored_verbatim() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 400,
            ..Default::default()
        });
        let packed = [0u64, 100, 10, 10, 100, 250, 0, 25, 250, 400, 5, 5];

        assert!(set_comp_ranges_with(
            &ops,
            host,
            1,
            2,
            packed.as_ptr(),
            packed.len(),
            true
        ));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_COMP_RANGES);
        assert!(guard.applied[0].managed);
        assert_eq!(guard.applied[0].blob_len, std::mem::size_of_val(&packed));
        let stored = unsafe {
            slice::from_raw_parts(guard.applied[0].blob.cast::<u64>(), packed.len())
        };
        assert_eq!(stored, packed);
    }

    #[test]
    fn comp_range_batch_rejects_malformed_or_locked_batches() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            ..Default::default()
        });

        // A trailing word that is not a full range.
        let ragged = [0u64, 50, 0, 0, 1];
        assert!(!set_comp_ranges_with(
            &ops,
            host,
            1,
            2,
            ragged.as_ptr(),
            ragged.len(),
            true
        ));
        // Ordered but past the region length.
        let beyond = [0u64, 101, 0, 0];
        assert!(!set_comp_ranges_with(
            &ops,
            host,
            1,
            2,
            beyond.as_ptr(),
            beyond.len(),
            true
        ));
        // A null pointer with a non-zero count must not be read.
        assert!(!set_comp_ranges_with(&ops, host, 1, 2, ptr::null(), 4, true));
        assert!(fake().lock().unwrap().applied.is_empty());

        // Locking only refuses turning the managed flag on.
        fake().lock().unwrap().state.locked = true;
        let valid = [0u64, 50, 0, 0];
        assert!(!set_comp_ranges_with(
            &ops,
            host,
            1,
            2,
            valid.as_ptr(),
            valid.len(),
            true
        ));
        assert!(set_comp_ranges_with(
            &ops,
            host,
            1,
            2,
            valid.as_ptr(),
            valid.len(),
            false
        ));
        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert!(!guard.applied[0].managed);
    }

    #[test]
    fn replace_audio_decodes_and_records_the_snapshot_pair() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            ..Default::default()
        });
        RELEASED_AUDIO.store(0, std::sync::atomic::Ordering::SeqCst);
        FAIL_DECODE.store(false, std::sync::atomic::Ordering::SeqCst);
        FAKE_RATE_BITS.store(48_000f64.to_bits(), std::sync::atomic::Ordering::SeqCst);

        let path = b"/tmp/replacement.wav";
        assert!(replace_audio_with(&ops, host, 1, 2, path));

        let guard = fake().lock().unwrap();
        assert_eq!(guard.applied.len(), 1);
        assert_eq!(guard.applied[0].op, HIRARI_REGION_EDIT_REPLACE_AUDIO);
        let blob = &guard.applied_blobs[0];
        assert_eq!(blob.len(), REPLACE_AUDIO_HEADER_BYTES + path.len());
        let raw = usize::from_ne_bytes(blob[0..8].try_into().unwrap());
        assert_eq!(raw, &FAKE_AUDIO as *const u8 as usize);
        assert_eq!(
            f64::from_ne_bytes(blob[8..16].try_into().unwrap()),
            44_100.0
        );
        assert_eq!(
            f64::from_ne_bytes(blob[16..24].try_into().unwrap()),
            48_000.0
        );
        assert_eq!(&blob[REPLACE_AUDIO_HEADER_BYTES..], path);
        assert_eq!(guard.undo_name, "Replace Region Audio");
        assert!(!guard.coalesce);
        assert!(!guard.perform);
        assert_eq!(guard.undo[0].op, HIRARI_REGION_EDIT_RESTORE);
        assert_eq!(guard.redo[0].op, HIRARI_REGION_EDIT_RESTORE);
        drop(guard);
        // The handle moved into the payload, so it is not released here.
        assert_eq!(RELEASED_AUDIO.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn replace_audio_rejects_bad_paths_files_regions_and_rates() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            len: 100,
            ..Default::default()
        });
        RELEASED_AUDIO.store(0, std::sync::atomic::Ordering::SeqCst);
        FAKE_RATE_BITS.store(48_000f64.to_bits(), std::sync::atomic::Ordering::SeqCst);

        assert!(!replace_audio_with(&ops, host, 1, 2, b""));
        FAIL_DECODE.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(!replace_audio_with(&ops, host, 1, 2, b"/tmp/missing.wav"));
        FAIL_DECODE.store(false, std::sync::atomic::Ordering::SeqCst);

        // The file decodes but the region is gone: the handle is released.
        fake().lock().unwrap().present = false;
        assert!(!replace_audio_with(&ops, host, 1, 2, b"/tmp/x.wav"));
        assert_eq!(RELEASED_AUDIO.load(std::sync::atomic::Ordering::SeqCst), 1);

        // A timeline rate the native setter refused before the swap.
        fake().lock().unwrap().present = true;
        FAKE_RATE_BITS.store(0f64.to_bits(), std::sync::atomic::Ordering::SeqCst);
        assert!(!replace_audio_with(&ops, host, 1, 2, b"/tmp/x.wav"));
        assert_eq!(RELEASED_AUDIO.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(fake().lock().unwrap().applied.is_empty());
        FAKE_RATE_BITS.store(48_000f64.to_bits(), std::sync::atomic::Ordering::SeqCst);
    }

    fn splittable_state() -> HirariRegionState {
        HirariRegionState {
            len: 1000,
            source_length: 1000,
            loop_count: 1,
            warp_ratio: 1.0,
            source_sample_rate: 48_000.0,
            timeline_sample_rate: 48_000.0,
            ..Default::default()
        }
    }

    #[test]
    fn split_rejects_positions_outside_the_region() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            start: 100,
            ..splittable_state()
        });
        for beat in [f64::NAN, f64::INFINITY, -1.0, 50.5, 100.0, 1100.0, 1200.0] {
            assert_eq!(
                split_region_with(&ops, host, 1, 2, beat),
                0,
                "beat {beat} must be rejected"
            );
        }
        let guard = fake().lock().unwrap();
        assert!(guard.applied.is_empty());
        assert!(guard.cloned_states.is_empty());
        assert!(guard.undo.is_empty());
    }

    #[test]
    fn split_applies_both_sides_and_records_the_restore_pair() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            start: 100,
            ..splittable_state()
        });
        RELEASED.store(0, std::sync::atomic::Ordering::SeqCst);
        {
            let mut guard = fake().lock().unwrap();
            guard.comp_managed = true;
            guard.comp_ranges = vec![NativeCompRange {
                start: 0,
                end: 500,
                fade_in: 0,
                fade_out: 0,
            }];
            guard.warp_markers = vec![HirariWarpMarker {
                source_sample: 0,
                timeline_sample: 0,
                transient: 0,
            }];
        }

        let right = split_region_with(&ops, host, 1, 2, 600.0);
        assert_eq!(right, 100);

        let guard = fake().lock().unwrap();
        // The clone starts from the original payload; the right-hand state
        // arrives with the first payload op.
        assert_eq!(guard.cloned_states, vec![HirariRegionState::default()]);
        let ops_applied: Vec<i32> = guard.applied.iter().map(|payload| payload.op).collect();
        assert_eq!(
            ops_applied,
            vec![
                HIRARI_REGION_EDIT_STATE,
                HIRARI_REGION_EDIT_MARKERS,
                HIRARI_REGION_EDIT_RANGE_EDITS,
                HIRARI_REGION_EDIT_COMP_RANGES,
                HIRARI_REGION_EDIT_NOTE_STORE,
                HIRARI_REGION_EDIT_STATE,
                HIRARI_REGION_EDIT_MARKERS,
                HIRARI_REGION_EDIT_RANGE_EDITS,
                HIRARI_REGION_EDIT_COMP_RANGES,
                HIRARI_REGION_EDIT_NOTE_STORE,
            ]
        );
        assert_eq!(guard.applied[0].state.start, 100);
        assert_eq!(guard.applied[0].state.len, 500);
        assert_eq!(guard.applied[5].state.start, 600);
        assert_eq!(guard.applied[5].state.len, 500);
        // Both sides keep the region's managed flag.
        assert!(guard.applied[3].managed);
        assert!(guard.applied[8].managed);
        assert_eq!(guard.undo_name, "Split Region");
        assert!(guard.coalesce);
        assert!(!guard.perform);
        assert_eq!(guard.undo.len(), 2);
        assert_eq!(guard.undo[0].op, HIRARI_REGION_EDIT_RESTORE);
        assert_eq!(guard.undo[1].op, HIRARI_REGION_EDIT_REMOVE);
        assert_eq!(guard.undo[1].region_id, 100);
        assert_eq!(guard.redo.len(), 2);
        assert_eq!(guard.redo[0].op, HIRARI_REGION_EDIT_RESTORE);
        assert_eq!(guard.redo[1].op, HIRARI_REGION_EDIT_RESTORE);
        assert_eq!(guard.redo[1].region_id, 100);
        drop(guard);
        assert_eq!(RELEASED.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn duplicate_copies_the_policy_state_and_records_remove_restore() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        reset(HirariRegionState {
            base_start: 100,
            base_length: 1000,
            source_offset: 100,
            ..splittable_state()
        });
        RELEASED.store(0, std::sync::atomic::Ordering::SeqCst);

        let id = duplicate_region_with(&ops, host, 1, 2, 5000);
        assert_eq!(id, 100);

        let guard = fake().lock().unwrap();
        assert_eq!(guard.cloned_states.len(), 1);
        assert_eq!(guard.cloned_states[0].start, 5000);
        // The untrimmed base backs up by the ceil of the trimmed span.
        assert_eq!(guard.cloned_states[0].base_start, 4900);
        assert!(guard.applied.is_empty());
        assert_eq!(guard.undo_name, "Duplicate Region");
        assert!(guard.coalesce);
        assert!(!guard.perform);
        assert_eq!(guard.undo.len(), 1);
        assert_eq!(guard.undo[0].op, HIRARI_REGION_EDIT_REMOVE);
        assert_eq!(guard.undo[0].region_id, 100);
        assert_eq!(guard.redo.len(), 1);
        assert_eq!(guard.redo[0].op, HIRARI_REGION_EDIT_RESTORE);
        drop(guard);
        assert_eq!(RELEASED.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn duplicate_rejects_an_empty_or_loopless_region() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ops = fake_ops();
        let host = 1usize as *mut c_void;
        for state in [
            HirariRegionState {
                len: 0,
                ..splittable_state()
            },
            HirariRegionState {
                loop_count: 0,
                ..splittable_state()
            },
        ] {
            reset(state);
            assert_eq!(duplicate_region_with(&ops, host, 1, 2, 5000), 0);
            let guard = fake().lock().unwrap();
            assert!(guard.cloned_states.is_empty());
            assert!(guard.undo.is_empty());
            drop(guard);
        }
    }
}
