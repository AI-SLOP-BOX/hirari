//! Region split/duplicate execution, split out of region_edit_host.rs.
use super::*;

pub(in crate::region_edit_host) struct SplitPlan(HirariRegionSplitOutput);

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
pub(in crate::region_edit_host) fn apply_split_side(
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
            generation: 0,
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
pub(in crate::region_edit_host) fn split_region_with(
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
    let right_id = crate::id_host::next_region_id(ops.list_tracks, ops.list_regions, host);
    if right_id == 0 || unsafe { clone(host, track_id, region_id, ptr::null(), right_id) } == 0 {
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
    if can_record(ops) {
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
pub(in crate::region_edit_host) fn duplicate_region_with(
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
    let duplicate_id = crate::id_host::next_region_id(ops.list_tracks, ops.list_regions, host);
    if duplicate_id == 0
        || unsafe { clone(host, track_id, region_id, &duplicate, duplicate_id) } == 0
    {
        return 0;
    }
    if !can_record(ops) {
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

