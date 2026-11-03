//! Per-region range-edit, comp-range and warp-marker storage.
//!
//! These three tables used to live as `std::vector`s inside `Region`. They are
//! edit-domain data read by the audio callback and by the layout serializer,
//! so moving them here needs a frozen projection: the control side mutates the
//! store, then publishes an immutable copy that the audio thread reads without
//! a lock. The frozen copy owns the buffers and the view struct, so shrinking
//! or replacing a table afterwards never invalidates a snapshot already
//! published.

use crate::region_comp_ranges::NativeCompRange;
use crate::region_processing::NativeRangeEdit;
use crate::region_warp::HirariWarpMarker;

const MAX_RANGE_EDITS: usize = 4096;
const MAX_COMP_RANGES: usize = 4096;
const MAX_WARP_MARKERS: usize = 100_000;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RegionClipEditsView {
    pub range_edits: *const NativeRangeEdit,
    pub range_edit_count: usize,
    pub comp_ranges: *const NativeCompRange,
    pub comp_range_count: usize,
    pub warp_markers: *const HirariWarpMarker,
    pub warp_marker_count: usize,
    pub comp_managed: u8,
}

pub struct RegionClipEdits {
    range_edits: Vec<NativeRangeEdit>,
    comp_ranges: Vec<NativeCompRange>,
    warp_markers: Vec<HirariWarpMarker>,
    comp_managed: bool,
}

impl RegionClipEdits {
    fn new() -> Self {
        Self {
            range_edits: Vec::new(),
            comp_ranges: Vec::new(),
            warp_markers: Vec::new(),
            comp_managed: false,
        }
    }

    fn clone_store(&self) -> Self {
        Self {
            range_edits: self.range_edits.clone(),
            comp_ranges: self.comp_ranges.clone(),
            warp_markers: self.warp_markers.clone(),
            comp_managed: self.comp_managed,
        }
    }
}

impl Default for RegionClipEdits {
    fn default() -> Self {
        Self::new()
    }
}

pub struct RegionClipEditsFrozen {
    // The view addresses the buffers below, so it must be built from them and
    // never outlive them; both are owned here to keep that true.
    pub view: RegionClipEditsView,
    _range_edits: Box<[NativeRangeEdit]>,
    _comp_ranges: Box<[NativeCompRange]>,
    _warp_markers: Box<[HirariWarpMarker]>,
}

/// Normalizes a warp-marker list through the shared Rust transform.
/// Returns `None` when the transform rejects the input outright.
fn normalized_warp_markers(
    markers: &[HirariWarpMarker],
    source_span: u64,
    timeline_length: u64,
) -> Option<Vec<HirariWarpMarker>> {
    if markers.is_empty() {
        return Some(Vec::new());
    }
    if timeline_length == 0 {
        return None;
    }
    let mut produced = 0usize;
    let raw = unsafe {
        crate::region_warp::hirari_region_normalize_warp_markers(
            markers.as_ptr(),
            markers.len(),
            source_span,
            timeline_length,
            &mut produced,
        )
    };
    if raw.is_null() {
        return None;
    }
    let adopted = unsafe { std::slice::from_raw_parts(raw, produced) }.to_vec();
    unsafe { crate::region_warp::hirari_free_warp_markers(raw, produced) };
    Some(adopted)
}

/// Applies the shared range-edit transform. `op` 0 normalizes (dropping
/// malformed entries), 3 re-admits a full replacement list.
fn transformed_range_edits(
    edits: &[NativeRangeEdit],
    region_length: u64,
    op: u32,
) -> Option<Vec<NativeRangeEdit>> {
    let mut produced = 0usize;
    let raw = unsafe {
        crate::region_processing::hirari_region_range_edits_apply(
            edits.as_ptr(),
            edits.len(),
            region_length,
            op,
            0,
            0,
            0.0,
            0,
            0,
            &mut produced,
        )
    };
    if raw.is_null() {
        return None;
    }
    let adopted = unsafe { std::slice::from_raw_parts(raw, produced) }.to_vec();
    unsafe { crate::region_processing::hirari_free_range_edits(raw, produced) };
    Some(adopted)
}

#[no_mangle]
pub extern "C" fn hirari_region_clip_edits_create() -> *mut RegionClipEdits {
    Box::into_raw(Box::new(RegionClipEdits::new()))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_clip_edits_destroy(store: *mut RegionClipEdits) {
    if store.is_null() {
        return;
    }
    unsafe { drop(Box::from_raw(store)) };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_clip_edits_clone(
    store: *const RegionClipEdits,
) -> *mut RegionClipEdits {
    if store.is_null() {
        return std::ptr::null_mut();
    }
    let copy = unsafe { &*store }.clone_store();
    Box::into_raw(Box::new(copy))
}

/// `validate` runs the replacement batch through the shared admission
/// transform; without it the list is stored verbatim (the caller already
/// produced a normalized batch).
///
/// # Safety
/// When `count > 0`, `edits` must be readable for `count` elements.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_clip_edits_replace_range_edits(
    store: *mut RegionClipEdits,
    edits: *const NativeRangeEdit,
    count: usize,
    region_length: u64,
    validate: bool,
) -> bool {
    if store.is_null() || count > MAX_RANGE_EDITS {
        return false;
    }
    let source: &[NativeRangeEdit] = if count == 0 {
        &[]
    } else if edits.is_null() {
        return false;
    } else {
        unsafe { std::slice::from_raw_parts(edits, count) }
    };
    let replacement = if validate {
        match transformed_range_edits(source, region_length, 3) {
            Some(next) => next,
            None => return false,
        }
    } else {
        source.to_vec()
    };
    unsafe { &mut *store }.range_edits = replacement;
    true
}

/// Re-normalizes the stored list in place after a region's geometry changed.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_clip_edits_normalize_range_edits(
    store: *mut RegionClipEdits,
    region_length: u64,
) {
    if store.is_null() {
        return;
    }
    let store = unsafe { &mut *store };
    if let Some(next) = transformed_range_edits(&store.range_edits, region_length, 0) {
        store.range_edits = next;
    }
}

/// # Safety
/// When `count > 0`, `ranges` must be readable for `count` elements.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_clip_edits_replace_comp_ranges(
    store: *mut RegionClipEdits,
    ranges: *const NativeCompRange,
    count: usize,
    managed: bool,
) -> bool {
    if store.is_null() || count > MAX_COMP_RANGES {
        return false;
    }
    if count != 0 && ranges.is_null() {
        return false;
    }
    let replacement = if count == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(ranges, count) }.to_vec()
    };
    let store = unsafe { &mut *store };
    store.comp_ranges = replacement;
    store.comp_managed = managed;
    true
}

/// Replaces the warp-marker list. With `normalize`, every marker must survive
/// the shared transform (the setter contract); without it the list is stored
/// as Rust already produced it.
///
/// Returns -1 when rejected, 0 when the list is unchanged, 1 when replaced.
///
/// # Safety
/// When `count > 0`, `markers` must be readable for `count` elements.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_clip_edits_set_warp_markers(
    store: *mut RegionClipEdits,
    markers: *const HirariWarpMarker,
    count: usize,
    source_span: u64,
    timeline_length: u64,
    normalize: bool,
) -> i32 {
    if store.is_null() || count > MAX_WARP_MARKERS {
        return -1;
    }
    let source: &[HirariWarpMarker] = if count == 0 {
        &[]
    } else if markers.is_null() {
        return -1;
    } else {
        unsafe { std::slice::from_raw_parts(markers, count) }
    };
    let store = unsafe { &mut *store };
    let replacement = if normalize {
        match normalized_warp_markers(source, source_span, timeline_length) {
            Some(next) if next.len() == source.len() => next,
            _ => return -1,
        }
    } else {
        source.to_vec()
    };
    if store.warp_markers == replacement {
        return 0;
    }
    store.warp_markers = replacement;
    1
}

/// Normalizes the stored markers, dropping whatever the transform rejects.
/// Region-add uses this: a malformed project may not inject markers, but the
/// region itself is still kept.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_clip_edits_normalize_warp_markers(
    store: *mut RegionClipEdits,
    source_span: u64,
    timeline_length: u64,
) {
    if store.is_null() {
        return;
    }
    let store = unsafe { &mut *store };
    if store.warp_markers.len() > MAX_WARP_MARKERS {
        store.warp_markers.truncate(MAX_WARP_MARKERS);
    }
    if store.warp_markers.is_empty() {
        return;
    }
    if let Some(next) = normalized_warp_markers(&store.warp_markers, source_span, timeline_length) {
        store.warp_markers = next;
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_clip_edits_freeze(
    store: *const RegionClipEdits,
) -> *mut RegionClipEditsFrozen {
    if store.is_null() {
        return std::ptr::null_mut();
    }
    let store = unsafe { &*store };
    let range_edits = store.range_edits.clone().into_boxed_slice();
    let comp_ranges = store.comp_ranges.clone().into_boxed_slice();
    let warp_markers = store.warp_markers.clone().into_boxed_slice();
    let view = RegionClipEditsView {
        range_edits: range_edits.as_ptr(),
        range_edit_count: range_edits.len(),
        comp_ranges: comp_ranges.as_ptr(),
        comp_range_count: comp_ranges.len(),
        warp_markers: warp_markers.as_ptr(),
        warp_marker_count: warp_markers.len(),
        comp_managed: u8::from(store.comp_managed),
    };
    Box::into_raw(Box::new(RegionClipEditsFrozen {
        view,
        _range_edits: range_edits,
        _comp_ranges: comp_ranges,
        _warp_markers: warp_markers,
    }))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_clip_edits_frozen_view(
    frozen: *const RegionClipEditsFrozen,
) -> *const RegionClipEditsView {
    if frozen.is_null() {
        return std::ptr::null();
    }
    unsafe { &(*frozen).view }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_clip_edits_frozen_free(
    frozen: *mut RegionClipEditsFrozen,
) {
    if frozen.is_null() {
        return;
    }
    unsafe { drop(Box::from_raw(frozen)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(start: u64, end: u64, gain: f32, fade_in: u64, fade_out: u64) -> NativeRangeEdit {
        NativeRangeEdit {
            start,
            end,
            gain,
            fade_in,
            fade_out,
        }
    }

    fn marker(source: u64, timeline: u64) -> HirariWarpMarker {
        HirariWarpMarker {
            source_sample: source,
            timeline_sample: timeline,
            transient: 0,
        }
    }

    #[test]
    fn normalized_range_edits_drop_malformed_and_clamp_fades() {
        let store = hirari_region_clip_edits_create();
        let raw = [
            edit(0, 100, 0.5, 999, 999),
            edit(8, 12, 1.0, 0, 0),
            edit(1, 2, f32::NAN, 0, 0),
        ];
        assert!(unsafe {
            hirari_region_clip_edits_replace_range_edits(store, raw.as_ptr(), raw.len(), 8, false)
        });
        unsafe { hirari_region_clip_edits_normalize_range_edits(store, 8) };
        // The validated setter must reject a batch it would have to trim.
        assert!(!unsafe {
            hirari_region_clip_edits_replace_range_edits(store, raw.as_ptr(), raw.len(), 8, true)
        });
        let frozen = unsafe { hirari_region_clip_edits_freeze(store) };
        let view = unsafe { &*hirari_region_clip_edits_frozen_view(frozen) };
        assert_eq!(view.range_edit_count, 1);
        let kept = unsafe { *view.range_edits };
        assert_eq!(kept.end, 8);
        assert_eq!(kept.fade_in, 8);
        assert_eq!(kept.fade_out, 8);
        unsafe {
            hirari_region_clip_edits_frozen_free(frozen);
            hirari_region_clip_edits_destroy(store);
        }
    }

    #[test]
    fn a_validated_batch_clamps_fades_and_a_leaving_span_is_refused() {
        let store = hirari_region_clip_edits_create();
        let inside = [edit(0, 4, 1.0, 99, 99)];
        assert!(unsafe {
            hirari_region_clip_edits_replace_range_edits(store, inside.as_ptr(), 1, 8, true)
        });
        let beyond = [edit(0, 9, 1.0, 0, 0)];
        assert!(!unsafe {
            hirari_region_clip_edits_replace_range_edits(store, beyond.as_ptr(), 1, 8, true)
        });
        let frozen = unsafe { hirari_region_clip_edits_freeze(store) };
        let view = unsafe { &*hirari_region_clip_edits_frozen_view(frozen) };
        assert_eq!(view.range_edit_count, 1);
        let kept = unsafe { *view.range_edits };
        assert_eq!(kept.end, 4);
        assert_eq!(kept.fade_in, 4);
        assert_eq!(kept.fade_out, 4);
        unsafe {
            hirari_region_clip_edits_frozen_free(frozen);
            hirari_region_clip_edits_destroy(store);
        }
    }

    #[test]
    fn unvalidated_range_edits_are_stored_verbatim() {
        let store = hirari_region_clip_edits_create();
        let raw = [edit(50, 20, 1.0, 0, 0), edit(1, 2, 1.0, 0, 0)];
        assert!(unsafe {
            hirari_region_clip_edits_replace_range_edits(store, raw.as_ptr(), raw.len(), 8, false)
        });
        let frozen = unsafe { hirari_region_clip_edits_freeze(store) };
        let view = unsafe { &*hirari_region_clip_edits_frozen_view(frozen) };
        assert_eq!(view.range_edit_count, 2);
        assert_eq!(unsafe { *view.range_edits }.start, 50);
        unsafe {
            hirari_region_clip_edits_frozen_free(frozen);
            hirari_region_clip_edits_destroy(store);
        }
    }

    #[test]
    fn warp_setter_rejects_lossy_normalization_and_reports_unchanged() {
        let store = hirari_region_clip_edits_create();
        let good = [marker(0, 0), marker(10, 20)];
        assert_eq!(
            unsafe {
                hirari_region_clip_edits_set_warp_markers(
                    store,
                    good.as_ptr(),
                    good.len(),
                    100,
                    100,
                    true,
                )
            },
            1
        );
        assert_eq!(
            unsafe {
                hirari_region_clip_edits_set_warp_markers(
                    store,
                    good.as_ptr(),
                    good.len(),
                    100,
                    100,
                    true,
                )
            },
            0
        );
        // A duplicate timeline anchor is dropped, so the setter must reject.
        let lossy = [marker(0, 0), marker(5, 10), marker(9, 10)];
        assert_eq!(
            unsafe {
                hirari_region_clip_edits_set_warp_markers(
                    store,
                    lossy.as_ptr(),
                    lossy.len(),
                    100,
                    100,
                    true,
                )
            },
            -1
        );
        unsafe { hirari_region_clip_edits_destroy(store) };
    }

    #[test]
    fn region_add_normalization_keeps_survivors_and_truncates() {
        let store = hirari_region_clip_edits_create();
        let raw = [marker(0, 0), marker(5, 10), marker(9, 10)];
        assert!(unsafe {
            hirari_region_clip_edits_set_warp_markers(
                store,
                raw.as_ptr(),
                raw.len(),
                0,
                0,
                false,
            ) >= 0
        });
        unsafe { hirari_region_clip_edits_normalize_warp_markers(store, 100, 100) };
        let frozen = unsafe { hirari_region_clip_edits_freeze(store) };
        let view = unsafe { &*hirari_region_clip_edits_frozen_view(frozen) };
        assert_eq!(view.warp_marker_count, 2);
        unsafe {
            hirari_region_clip_edits_frozen_free(frozen);
            hirari_region_clip_edits_destroy(store);
        }
    }

    #[test]
    fn frozen_projection_survives_store_mutation() {
        let store = hirari_region_clip_edits_create();
        let raw = [edit(0, 4, 1.0, 0, 0)];
        assert!(unsafe {
            hirari_region_clip_edits_replace_range_edits(store, raw.as_ptr(), raw.len(), 8, false)
        });
        let frozen = unsafe { hirari_region_clip_edits_freeze(store) };
        assert!(unsafe {
            hirari_region_clip_edits_replace_range_edits(store, std::ptr::null(), 0, 8, false)
        });
        let view = unsafe { &*hirari_region_clip_edits_frozen_view(frozen) };
        assert_eq!(view.range_edit_count, 1);
        assert_eq!(unsafe { *view.range_edits }.end, 4);
        unsafe {
            hirari_region_clip_edits_frozen_free(frozen);
            hirari_region_clip_edits_destroy(store);
        }
    }

    #[test]
    fn comp_ranges_and_managed_flag_round_trip() {
        let store = hirari_region_clip_edits_create();
        let ranges = [
            NativeCompRange {
                start: 0,
                end: 10,
                fade_in: 1,
                fade_out: 1,
            },
            NativeCompRange {
                start: 10,
                end: 20,
                fade_in: 0,
                fade_out: 2,
            },
        ];
        assert!(unsafe {
            hirari_region_clip_edits_replace_comp_ranges(
                store,
                ranges.as_ptr(),
                ranges.len(),
                true,
            )
        });
        let frozen = unsafe { hirari_region_clip_edits_freeze(store) };
        let view = unsafe { &*hirari_region_clip_edits_frozen_view(frozen) };
        assert_eq!(view.comp_range_count, 2);
        assert_eq!(view.comp_managed, 1);
        assert_eq!(unsafe { *view.comp_ranges.add(1) }.end, 20);
        unsafe {
            hirari_region_clip_edits_frozen_free(frozen);
            hirari_region_clip_edits_destroy(store);
        }
    }

    #[test]
    fn clone_is_independent_of_its_source() {
        let store = hirari_region_clip_edits_create();
        let raw = [edit(0, 4, 1.0, 0, 0)];
        assert!(unsafe {
            hirari_region_clip_edits_replace_range_edits(store, raw.as_ptr(), raw.len(), 8, false)
        });
        let copy = unsafe { hirari_region_clip_edits_clone(store) };
        assert!(unsafe {
            hirari_region_clip_edits_replace_range_edits(copy, std::ptr::null(), 0, 8, false)
        });
        let original = unsafe { hirari_region_clip_edits_freeze(store) };
        let cloned = unsafe { hirari_region_clip_edits_freeze(copy) };
        let original_view = unsafe { &*hirari_region_clip_edits_frozen_view(original) };
        let cloned_view = unsafe { &*hirari_region_clip_edits_frozen_view(cloned) };
        assert_eq!(original_view.range_edit_count, 1);
        assert_eq!(cloned_view.range_edit_count, 0);
        unsafe {
            hirari_region_clip_edits_frozen_free(original);
            hirari_region_clip_edits_frozen_free(cloned);
            hirari_region_clip_edits_destroy(copy);
            hirari_region_clip_edits_destroy(store);
        }
    }

    #[test]
    fn rejects_null_store_and_oversized_batches() {
        let bogus = edit(0, 1, 1.0, 0, 0);
        assert!(!unsafe {
            hirari_region_clip_edits_replace_range_edits(
                std::ptr::null_mut(),
                &bogus,
                1,
                8,
                false,
            )
        });
        let store = hirari_region_clip_edits_create();
        assert!(!unsafe {
            hirari_region_clip_edits_replace_range_edits(
                store,
                std::ptr::null(),
                1,
                8,
                false,
            )
        });
        assert!(!unsafe {
            hirari_region_clip_edits_replace_comp_ranges(
                store,
                std::ptr::null(),
                MAX_COMP_RANGES + 1,
                false,
            )
        });
        assert_eq!(
            unsafe {
                hirari_region_clip_edits_set_warp_markers(
                    store,
                    std::ptr::null(),
                    1,
                    0,
                    0,
                    true,
                )
            },
            -1
        );
        unsafe { hirari_region_clip_edits_destroy(store) };
    }
}
