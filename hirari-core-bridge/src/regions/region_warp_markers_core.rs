//! Warp marker read side, split out of region_edit_host.rs.
use super::*;

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
pub(in crate::region_edit_host) fn normalize_markers(
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

pub(in crate::region_edit_host) fn same_markers(left: &[HirariWarpMarker], right: &[HirariWarpMarker]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(a, b)| {
            a.source_sample == b.source_sample
                && a.timeline_sample == b.timeline_sample
                && a.transient == b.transient
        })
}

/// The previous engine setter rejected edits above this order of magnitude.
pub(super) const MAX_WARP_MARKER_EDITS: usize = 100_000;

/// Replaces a region's warp markers. The previous engine setter rejected any
/// list that normalization would shorten, so callers can rely on a marker
/// count round-trip; a successful edit records one marker-aware undo entry.
pub(crate) fn set_warp_markers_with(
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
