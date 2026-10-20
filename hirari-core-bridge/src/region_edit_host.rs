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
    pub state: HirariRegionState,
}

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
pub type HirariRegionRecordUndoFn = unsafe extern "C" fn(
    *mut c_void,
    *const c_char,
    *const HirariRegionEditPayload,
    usize,
    *const HirariRegionEditPayload,
    usize,
);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariRegionHostOps {
    pub find_region: Option<HirariRegionFindFn>,
    pub apply_edit: Option<HirariRegionApplyFn>,
    pub list_sync_group: Option<HirariRegionListGroupFn>,
    pub record_undo: Option<HirariRegionRecordUndoFn>,
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
        _ => return None,
    })
}

/// Undo labels, kept identical to the strings the C++ layer used to record.
fn field_name(field: HirariRegionScalarField) -> &'static [u8] {
    match field {
        HirariRegionScalarField::ClipGain => b"Region Gain\0",
        HirariRegionScalarField::Muted => b"Mute Region\0",
        HirariRegionScalarField::Reverse => b"Reverse Region\0",
        HirariRegionScalarField::PitchSemitones => b"Flex Pitch\0",
        HirariRegionScalarField::LoopCount => b"Region Loop\0",
        HirariRegionScalarField::SyncGroup => b"Region Sync Group\0",
        HirariRegionScalarField::Fades => b"Region Fades\0",
    }
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
    name: &'static [u8],
    undo: &[HirariRegionEditPayload],
    redo: &[HirariRegionEditPayload],
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
    record(ops, host, field_name(field), &[before_payload], &[after_payload]);
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
    record(ops, host, b"Move Region\0", &[before], &[redo]);
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
    record(ops, host, b"Move Region Sync Group\0", &undo, &redo);
    true
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::slice;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeHost {
        present: bool,
        state: HirariRegionState,
        applied: Vec<HirariRegionEditPayload>,
        undo: Vec<HirariRegionEditPayload>,
        redo: Vec<HirariRegionEditPayload>,
        undo_name: String,
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
        let payload = unsafe { ptr::read(payload) };
        fake().lock().unwrap().applied.push(payload);
        true
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

    unsafe extern "C" fn fake_record_undo(
        _host: *mut c_void,
        name: *const c_char,
        undo: *const HirariRegionEditPayload,
        undo_count: usize,
        redo: *const HirariRegionEditPayload,
        redo_count: usize,
    ) {
        let mut guard = fake().lock().unwrap();
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
}
