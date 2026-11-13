//! Range edit and comp-range commit paths, split out of region_edit_host.rs.
use super::*;

pub(crate) fn set_range_edit_with(
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

pub(crate) fn clear_range_edits_with(
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
pub(crate) fn set_comp_ranges_with(
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
        generation: 0,
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
pub(crate) fn edit_move_with(
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
pub(crate) fn edit_move_sync_group_with(
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

