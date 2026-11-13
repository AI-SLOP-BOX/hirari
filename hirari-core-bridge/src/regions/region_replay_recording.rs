//! Replay/undo machinery for region edits: recording, draining and
//! re-applying the sequence of edits after a project load, split out of
//! region_edit_host.rs.
use super::*;

/// Applies the accepted decision to a copy of the current state. Only the
/// fields the policy controls are touched; geometry is left alone.
pub(crate) fn edited_state(
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
pub(crate) fn record_performed(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    name: &[u8],
    undo: &[HirariRegionEditPayload],
    redo: &[HirariRegionEditPayload],
) -> bool {
    record_with(ops, host, name, undo, redo, true, true)
}

/// Whether the host can take an undo entry. The engine exposes its Rust
/// history through `undo_state`; `record_undo` is the test seam that captures
/// payloads instead.
pub(crate) fn can_record(ops: &HirariRegionHostOps) -> bool {
    ops.record_undo.is_some() || (ops.undo_state.is_some() && ops.apply_edit.is_some())
}

/// Copied or adopted storage a payload needs at replay time, because the
/// caller's buffer and ownership handover expire once the edit returns.
enum OwnedBlob {
    Markers(Vec<HirariWarpMarker>),
    RangeEdits(Vec<NativeRangeEdit>),
}

struct OwnedEdit {
    payload: HirariRegionEditPayload,
    blob: Option<OwnedBlob>,
    // Note-table snapshot the caller handed over with a NoteStore payload.
    note_store: *mut HirariRegionVariAudioStore,
    // Whole-region snapshot the caller handed over with a Restore payload.
    snapshot: *mut c_void,
}

impl OwnedEdit {
    fn capture(source: HirariRegionEditPayload) -> Self {
        let mut edit = Self {
            payload: source,
            blob: None,
            note_store: ptr::null_mut(),
            snapshot: ptr::null_mut(),
        };
        if source.blob.is_null() {
            return edit;
        }
        match source.op {
            HIRARI_REGION_EDIT_NOTE_STORE => {
                edit.note_store = source.blob as *mut HirariRegionVariAudioStore;
            }
            HIRARI_REGION_EDIT_RESTORE => {
                edit.snapshot = source.blob as *mut c_void;
            }
            HIRARI_REGION_EDIT_GEOMETRY | HIRARI_REGION_EDIT_MARKERS => {
                let count = source.blob_len / std::mem::size_of::<HirariWarpMarker>();
                let markers =
                    unsafe { slice::from_raw_parts(source.blob.cast::<HirariWarpMarker>(), count) };
                edit.blob = Some(OwnedBlob::Markers(markers.to_vec()));
                edit.payload.blob_len = std::mem::size_of_val(markers);
            }
            HIRARI_REGION_EDIT_RANGE_EDITS => {
                let count = source.blob_len / std::mem::size_of::<NativeRangeEdit>();
                let edits =
                    unsafe { slice::from_raw_parts(source.blob.cast::<NativeRangeEdit>(), count) };
                edit.blob = Some(OwnedBlob::RangeEdits(edits.to_vec()));
                edit.payload.blob_len = std::mem::size_of_val(edits);
            }
            _ => {}
        }
        edit
    }

    fn replay(&self, ops: &HirariRegionHostOps, host: *mut c_void) {
        let Some(apply) = ops.apply_edit else { return; };
        let mut payload = self.payload;
        if !self.note_store.is_null() {
            payload.blob = self.note_store.cast();
            payload.blob_len = 0;
        }
        if !self.snapshot.is_null() {
            payload.blob = self.snapshot.cast();
            payload.blob_len = 0;
        }
        match &self.blob {
            Some(OwnedBlob::Markers(markers)) => {
                payload.blob = markers.as_ptr().cast();
                payload.blob_len = std::mem::size_of_val(markers.as_slice());
            }
            Some(OwnedBlob::RangeEdits(edits)) => {
                payload.blob = edits.as_ptr().cast();
                payload.blob_len = std::mem::size_of_val(edits.as_slice());
            }
            None => {}
        }
        unsafe { apply(host, &payload) };
    }
}

/// One history side. The history owns it until the entry is evicted, then
/// `destroy_entry` drops the adopted note tables and region snapshots.
struct ReplayEntry {
    ops: HirariRegionHostOps,
    host: *mut c_void,
    edits: Vec<OwnedEdit>,
}

impl Drop for ReplayEntry {
    fn drop(&mut self) {
        for edit in &self.edits {
            unsafe { vari_store_release(edit.note_store) };
            if !edit.snapshot.is_null() {
                if let Some(release) = self.ops.release_snapshot {
                    unsafe { release(self.host, edit.snapshot) };
                }
            }
        }
    }
}

unsafe extern "C" fn replay_entry(context: *mut c_void) {
    if context.is_null() {
        return;
    }
    let entry = unsafe { &*(context as *const ReplayEntry) };
    for edit in &entry.edits {
        edit.replay(&entry.ops, entry.host);
    }
}

unsafe extern "C" fn destroy_entry(context: *mut c_void) {
    if context.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(context as *mut ReplayEntry) });
}

pub(crate) fn register_entry(state: *mut c_void, entry: Box<ReplayEntry>) -> u64 {
    let context = Box::into_raw(entry).cast::<c_void>();
    let id = unsafe {
        hirari_undo_manager_register_native_callback(
            state,
            context,
            Some(replay_entry),
            Some(destroy_entry),
        )
    };
    if id == 0 {
        unsafe { drop(Box::from_raw(context as *mut ReplayEntry)) };
    }
    id
}

/// Records one history entry in the engine's own history, taking ownership of
/// the handles the caller attached to the payloads and copying the temporary
/// marker/range-edit buffers the payloads point at.
pub(crate) fn record_owned(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    name: &[u8],
    undo: &[HirariRegionEditPayload],
    redo: &[HirariRegionEditPayload],
    coalesce: bool,
    perform: bool,
) -> bool {
    let Some(read_state) = ops.undo_state else { return false; };
    let state = unsafe { read_state(host) };
    if state.is_null() {
        return false;
    }
    let undo_entry = Box::new(ReplayEntry {
        ops: *ops,
        host,
        edits: undo.iter().copied().map(OwnedEdit::capture).collect(),
    });
    let undo_id = register_entry(state, undo_entry);
    if undo_id == 0 {
        return false;
    }
    let redo_entry = Box::new(ReplayEntry {
        ops: *ops,
        host,
        edits: redo.iter().copied().map(OwnedEdit::capture).collect(),
    });
    let redo_id = register_entry(state, redo_entry);
    if redo_id == 0 {
        unsafe { hirari_undo_manager_discard_native_callback(state, undo_id) };
        return false;
    }
    let name = name.strip_suffix(&[0u8][..]).unwrap_or(name);
    unsafe {
        hirari_undo_manager_commit(
            state,
            name.as_ptr(),
            name.len(),
            undo_id,
            redo_id,
            coalesce,
            perform,
        )
    }
}

pub(crate) fn record_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    name: &[u8],
    undo: &[HirariRegionEditPayload],
    redo: &[HirariRegionEditPayload],
    coalesce: bool,
    perform: bool,
) -> bool {
    if let Some(record_undo) = ops.record_undo {
        return unsafe {
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
        };
    }
    record_owned(ops, host, name, undo, redo, coalesce, perform)
}

/// Applies one scalar field edit through an explicit host table.
///
/// Kept separate from the exported function so tests never have to touch the
/// process-global table while other tests are driving the native engine.
pub(crate) fn edit_scalar_with(
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
