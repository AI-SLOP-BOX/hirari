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
    hirari_region_vari_audio_store_clear_segments, hirari_region_vari_audio_store_remove,
    hirari_region_vari_audio_store_upsert, hirari_region_vari_audio_store_upsert_anchor,
    hirari_region_vari_audio_store_warp, vari_store_clone, vari_store_release, vari_store_restore,
    vari_store_view, HirariAudioNoteSegment, HirariRegionVariAudioStore,
};
use std::os::raw::c_void;
use std::ptr;
use std::sync::{Mutex, OnceLock};

use crate::region_mutations::hirari_region_sync_move_positions;
use crate::region_processing::{
    hirari_region_fade_samples, HirariRegionScalarDecision, HirariRegionScalarEdit,
    HirariRegionScalarField,
};

/// Payload op: write the policy-controlled scalar fields in place.
pub const HIRARI_REGION_EDIT_INPLACE: i32 = 2;
/// Payload op: replace the region's editable state and republish it.
pub const HIRARI_REGION_EDIT_STATE: i32 = 1;
/// Payload op: remove the region and drop its cached peaks.
pub const HIRARI_REGION_EDIT_REMOVE: i32 = 12;
/// Payload op: put a removed region back from a host-held snapshot handle. The
/// host takes ownership of the handle when the payload is recorded for undo.
pub const HIRARI_REGION_EDIT_RESTORE: i32 = 13;

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
    fn source_frames_per_timeline_frame(&self) -> f64 {
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
);
/// Copies the whole region (audio, history, note edits) into an owned host
/// handle, or returns null when the region does not exist.
pub type HirariRegionSnapshotFn = unsafe extern "C" fn(*mut c_void, u32, u32) -> *mut c_void;
/// Frees a handle from `HirariRegionSnapshotFn` that was never recorded.
pub type HirariRegionReleaseSnapshotFn = unsafe extern "C" fn(*mut c_void, *mut c_void);

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
}

// The table only holds function pointers supplied by the native engine; it
// carries no data the Rust side mutates.
unsafe impl Send for HirariRegionHostOps {}
unsafe impl Sync for HirariRegionHostOps {}

static HOST_OPS: OnceLock<Mutex<Option<HirariRegionHostOps>>> = OnceLock::new();

fn host_ops() -> Option<HirariRegionHostOps> {
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

fn record(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    name: &[u8],
    undo: &[HirariRegionEditPayload],
    redo: &[HirariRegionEditPayload],
    coalesce: bool,
) {
    if let Some(record_undo) = ops.record_undo {
        unsafe {
            record_undo(
                host,
                name.as_ptr() as *const c_char,
                undo.as_ptr(),
                undo.len(),
                redo.as_ptr(),
                redo.len(),
                coalesce,
            );
        }
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
    hirari_region_vari_audio_store_clear_segments, hirari_region_vari_audio_store_remove,
    hirari_region_vari_audio_store_upsert, hirari_region_vari_audio_store_upsert_anchor,
    hirari_region_vari_audio_store_warp, vari_store_clone, vari_store_release, vari_store_restore,
    vari_store_view, HirariAudioNoteSegment, HirariRegionVariAudioStore,
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
        undo: Vec<HirariRegionEditPayload>,
        redo: Vec<HirariRegionEditPayload>,
        undo_name: String,
        coalesce: bool,
        note_store: usize,
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
        fake().lock().unwrap().applied.push(payload);
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
    ) {
        let mut guard = fake().lock().unwrap();
        guard.coalesce = coalesce;
        guard.undo_name = unsafe { std::ffi::CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned();
        guard.undo = unsafe { slice::from_raw_parts(undo, undo_count) }.to_vec();
        guard.redo = unsafe { slice::from_raw_parts(redo, redo_count) }.to_vec();
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
}
