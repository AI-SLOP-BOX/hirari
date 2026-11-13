//! Geometry edit entry points: trim, warp-ratio, markers.
use super::*;

pub(crate) fn edit_trim_with(
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

pub(crate) fn edit_warp_ratio_with(
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
pub(crate) fn set_warp_markers_from_slices(
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

pub(crate) fn read_range_edits(
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

pub(crate) fn range_edit_payload(
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

