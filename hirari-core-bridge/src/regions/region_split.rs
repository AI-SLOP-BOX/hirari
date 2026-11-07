//! Region split and duplicate policy.
//!
//! The engine still owns the audio buffers and the immutable snapshot the
//! audio thread reads, but every timeline decision the old C++
//! `split_region_with_right_id` / `duplicate_region` made was a pure numeric
//! transform. Rust owns that policy here: fades, source-length and reverse
//! offset surgery, warp-map splitting, local range/comp windowing and the
//! duplicate trim. C++ clones the payload and stores the returned state.

use std::slice;

use crate::region_comp_ranges::NativeCompRange;
use crate::region_edit_host::HirariRegionState;
use crate::region_mutations::split_warp_markers;
use crate::region_processing::NativeRangeEdit;
use crate::region_warp::HirariWarpMarker;

const MAX_LOCAL_RANGES: usize = 4096;
const MAX_WARP_MARKERS: usize = 100_000;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HirariRegionSplitInput {
    pub state: HirariRegionState,
    pub split_offset: u64,
    pub markers: *const HirariWarpMarker,
    pub marker_count: usize,
    pub ranges: *const NativeRangeEdit,
    pub range_count: usize,
    pub comp_ranges: *const NativeCompRange,
    pub comp_range_count: usize,
}

/// Everything the caller needs to build the two region payloads. Buffers are
/// owned by Rust until `hirari_region_split_plan_free`; empty results are null.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariRegionSplitOutput {
    pub left: HirariRegionState,
    pub right: HirariRegionState,
    pub left_markers: *mut HirariWarpMarker,
    pub left_marker_count: usize,
    pub right_markers: *mut HirariWarpMarker,
    pub right_marker_count: usize,
    pub left_ranges: *mut NativeRangeEdit,
    pub left_range_count: usize,
    pub right_ranges: *mut NativeRangeEdit,
    pub right_range_count: usize,
    pub left_comp_ranges: *mut NativeCompRange,
    pub left_comp_range_count: usize,
    pub right_comp_ranges: *mut NativeCompRange,
    pub right_comp_range_count: usize,
}

fn boxed<T>(values: Vec<T>) -> (*mut T, usize) {
    if values.is_empty() {
        return (std::ptr::null_mut(), 0);
    }
    let count = values.len();
    let boxed = values.into_boxed_slice();
    (Box::into_raw(boxed) as *mut T, count)
}

unsafe fn free_boxed<T>(values: *mut T, count: usize) {
    if values.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(values, count)));
    }
}

/// Splits one region's editable state and local data at `split_offset`
/// timeline samples. Rejects every geometry that the C++ splitter rejected.
///
/// # Safety
/// `input` must be readable and `out` writable. Marker/range pointers must be
/// readable for their counts when the counts are non-zero.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_split_plan(
    input: *const HirariRegionSplitInput,
    out: *mut HirariRegionSplitOutput,
) -> bool {
    if input.is_null() || out.is_null() {
        return false;
    }
    let input = unsafe { &*input };
    if input.marker_count > MAX_WARP_MARKERS
        || input.range_count > MAX_LOCAL_RANGES
        || input.comp_range_count > MAX_LOCAL_RANGES
        || (input.marker_count != 0 && input.markers.is_null())
        || (input.range_count != 0 && input.ranges.is_null())
        || (input.comp_range_count != 0 && input.comp_ranges.is_null())
    {
        return false;
    }
    let state = input.state;
    if state.len == 0
        || input.split_offset == 0
        || input.split_offset >= state.len
        || state.start.checked_add(state.len).is_none()
    {
        return false;
    }
    let source_length = if state.source_length == 0 {
        state.len
    } else {
        state.source_length
    };
    if source_length <= 1 {
        return false;
    }
    let markers = if input.marker_count == 0 {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(input.markers, input.marker_count) }
    };
    let Some((left_markers, right_markers, source_split)) = split_warp_markers(
        markers,
        source_length,
        state.len,
        input.split_offset,
        state.source_frames_per_timeline_frame(),
    ) else {
        return false;
    };
    let right_source_length = source_length - source_split;
    if source_split == 0 || right_source_length == 0 {
        return false;
    }

    let mut left = state;
    let mut right = state;
    left.len = input.split_offset;
    right.start = state.start + input.split_offset;
    right.len = state.len - input.split_offset;
    left.fade_in_samples = state.fade_in_samples.min(left.len);
    left.fade_out_samples = 0;
    right.fade_in_samples = 0;
    right.fade_out_samples = state.fade_out_samples.min(right.len);
    left.source_length = source_split;
    right.source_length = right_source_length;
    if state.reverse {
        let Some(offset) = state.source_offset.checked_add(source_length - source_split) else {
            return false;
        };
        left.source_offset = offset;
    } else {
        let Some(offset) = state.source_offset.checked_add(source_split) else {
            return false;
        };
        right.source_offset = offset;
    }

    let mut left_ranges = Vec::with_capacity(input.range_count);
    let mut right_ranges = Vec::with_capacity(input.range_count);
    if input.range_count != 0 {
        let ranges = unsafe { slice::from_raw_parts(input.ranges, input.range_count) };
        split_range_edits(
            ranges,
            state.len,
            input.split_offset,
            &mut left_ranges,
            &mut right_ranges,
        );
    }
    let mut left_comp_ranges = Vec::with_capacity(input.comp_range_count);
    let mut right_comp_ranges = Vec::with_capacity(input.comp_range_count);
    if input.comp_range_count != 0 {
        let ranges =
            unsafe { slice::from_raw_parts(input.comp_ranges, input.comp_range_count) };
        split_comp_ranges(
            ranges,
            state.len,
            input.split_offset,
            &mut left_comp_ranges,
            &mut right_comp_ranges,
        );
    }

    let (left_markers, left_marker_count) = boxed(left_markers);
    let (right_markers, right_marker_count) = boxed(right_markers);
    let (left_ranges, left_range_count) = boxed(left_ranges);
    let (right_ranges, right_range_count) = boxed(right_ranges);
    let (left_comp_ranges, left_comp_range_count) = boxed(left_comp_ranges);
    let (right_comp_ranges, right_comp_range_count) = boxed(right_comp_ranges);
    unsafe {
        *out = HirariRegionSplitOutput {
            left,
            right,
            left_markers,
            left_marker_count,
            right_markers,
            right_marker_count,
            left_ranges,
            left_range_count,
            right_ranges,
            right_range_count,
            left_comp_ranges,
            left_comp_range_count,
            right_comp_ranges,
            right_comp_range_count,
        };
    }
    true
}

/// Releases the buffers produced by [`hirari_region_split_plan`].
///
/// # Safety
/// `out` must come from a successful `hirari_region_split_plan` call and must
/// not be freed twice.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_split_plan_free(out: *mut HirariRegionSplitOutput) {
    if out.is_null() {
        return;
    }
    let out = unsafe { &mut *out };
    unsafe {
        free_boxed(out.left_markers, out.left_marker_count);
        free_boxed(out.right_markers, out.right_marker_count);
        free_boxed(out.left_ranges, out.left_range_count);
        free_boxed(out.right_ranges, out.right_range_count);
        free_boxed(out.left_comp_ranges, out.left_comp_range_count);
        free_boxed(out.right_comp_ranges, out.right_comp_range_count);
    }
    *out = HirariRegionSplitOutput::default();
}

/// Applies the duplicate policy: keep every field, place the copy at
/// `start_sample`, and back the untrimmed base up by the ceil of the trimmed
/// source span.
///
/// # Safety
/// `state` must be readable and `out` writable.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_duplicate_state(
    state: *const HirariRegionState,
    start_sample: u64,
    out: *mut HirariRegionState,
) -> bool {
    if state.is_null() || out.is_null() {
        return false;
    }
    let state = unsafe { &*state };
    if state.len == 0 || state.loop_count == 0 {
        return false;
    }
    let Some(span) = state.len.checked_mul(state.loop_count as u64) else {
        return false;
    };
    if start_sample > u64::MAX - span {
        return false;
    }
    let source_trim = state.source_offset.saturating_sub(state.base_source_offset);
    let quotient = source_trim as f64 / state.source_frames_per_timeline_frame();
    if !quotient.is_finite() {
        return false;
    }
    let timeline_trim = quotient.ceil();
    if timeline_trim < 0.0 || timeline_trim > start_sample as f64 {
        return false;
    }
    let mut duplicate = *state;
    duplicate.start = start_sample;
    duplicate.base_start = start_sample - timeline_trim as u64;
    unsafe { *out = duplicate };
    true
}

fn split_range_edits(
    source: &[NativeRangeEdit],
    region_length: u64,
    split_offset: u64,
    left: &mut Vec<NativeRangeEdit>,
    right: &mut Vec<NativeRangeEdit>,
) {
    for range in source {
        let source_end = range.end.min(region_length);
        if range.start >= source_end {
            continue;
        }
        let left_end = source_end.min(split_offset);
        if range.start < left_end {
            let mut copy = *range;
            copy.start = range.start;
            copy.end = left_end;
            let length = left_end - range.start;
            copy.fade_in = range.fade_in.min(length);
            copy.fade_out = if left_end == range.end {
                range.fade_out.min(length)
            } else {
                0
            };
            left.push(copy);
        }
        let right_start = range.start.max(split_offset);
        if right_start < source_end {
            let mut copy = *range;
            copy.start = right_start - split_offset;
            copy.end = source_end - split_offset;
            let length = copy.end - copy.start;
            copy.fade_in = if right_start == range.start {
                range.fade_in.min(length)
            } else {
                0
            };
            copy.fade_out = if source_end == range.end {
                range.fade_out.min(length)
            } else {
                0
            };
            right.push(copy);
        }
    }
}

fn split_comp_ranges(
    source: &[NativeCompRange],
    region_length: u64,
    split_offset: u64,
    left: &mut Vec<NativeCompRange>,
    right: &mut Vec<NativeCompRange>,
) {
    for range in source {
        let source_end = range.end.min(region_length);
        if range.start >= source_end {
            continue;
        }
        let left_end = source_end.min(split_offset);
        if range.start < left_end {
            let mut copy = *range;
            copy.start = range.start;
            copy.end = left_end;
            let length = left_end - range.start;
            copy.fade_in = range.fade_in.min(length);
            copy.fade_out = if left_end == range.end {
                range.fade_out.min(length)
            } else {
                0
            };
            left.push(copy);
        }
        let right_start = range.start.max(split_offset);
        if right_start < source_end {
            let mut copy = *range;
            copy.start = right_start - split_offset;
            copy.end = source_end - split_offset;
            let length = copy.end - copy.start;
            copy.fade_in = if right_start == range.start {
                range.fade_in.min(length)
            } else {
                0
            };
            copy.fade_out = if source_end == range.end {
                range.fade_out.min(length)
            } else {
                0
            };
            right.push(copy);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vari_audio::{
        hirari_region_vari_audio_freeze, hirari_region_vari_audio_frozen_free,
        hirari_region_vari_audio_frozen_split, hirari_region_vari_audio_store_clear,
        hirari_region_vari_audio_store_new, hirari_region_vari_audio_store_split,
        hirari_region_vari_audio_store_upsert, hirari_region_vari_audio_store_upsert_anchor,
        HirariAudioNoteSegment, HirariRegionVariAudioStore,
    };

    fn state() -> HirariRegionState {
        HirariRegionState {
            start: 1_000,
            len: 1_000,
            base_start: 1_000,
            base_length: 1_000,
            source_offset: 10,
            base_source_offset: 10,
            source_length: 1_000,
            fade_in_samples: 100,
            fade_out_samples: 200,
            warp_ratio: 1.0,
            source_sample_rate: 1_000.0,
            timeline_sample_rate: 1_000.0,
            clip_gain: 1.0,
            loop_count: 1,
            ..Default::default()
        }
    }

    fn plan(input: &HirariRegionSplitInput) -> Option<HirariRegionSplitOutput> {
        let mut out = HirariRegionSplitOutput::default();
        if unsafe { hirari_region_split_plan(input, &mut out) } {
            Some(out)
        } else {
            None
        }
    }

    fn release(out: &mut HirariRegionSplitOutput) {
        unsafe { hirari_region_split_plan_free(out) };
    }

    #[test]
    fn split_moves_fades_to_the_outer_edges_and_advances_the_source() {
        let mut input = HirariRegionSplitInput {
            state: state(),
            split_offset: 400,
            markers: std::ptr::null(),
            marker_count: 0,
            ranges: std::ptr::null(),
            range_count: 0,
            comp_ranges: std::ptr::null(),
            comp_range_count: 0,
        };
        let mut out = plan(&input).expect("valid split");
        assert_eq!(out.left.start, 1_000);
        assert_eq!(out.left.len, 400);
        assert_eq!(out.left.fade_in_samples, 100);
        assert_eq!(out.left.fade_out_samples, 0);
        assert_eq!(out.left.source_length, 400);
        assert_eq!(out.left.source_offset, 10);
        assert_eq!(out.right.start, 1_400);
        assert_eq!(out.right.len, 600);
        assert_eq!(out.right.fade_in_samples, 0);
        assert_eq!(out.right.fade_out_samples, 200);
        assert_eq!(out.right.source_length, 600);
        assert_eq!(out.right.source_offset, 410);
        release(&mut out);

        input.state.reverse = true;
        let mut out = plan(&input).expect("valid reverse split");
        assert_eq!(out.left.source_offset, 610);
        assert_eq!(out.right.source_offset, 10);
        release(&mut out);
    }

    #[test]
    fn split_windows_range_and_comp_edits_to_each_half() {
        let ranges = [
            NativeRangeEdit {
                start: 0,
                end: 500,
                gain: 0.5,
                fade_in: 10,
                fade_out: 20,
            },
            NativeRangeEdit {
                start: 700,
                end: 900,
                gain: 2.0,
                fade_in: 0,
                fade_out: 0,
            },
        ];
        let comps = [NativeCompRange {
            start: 0,
            end: 500,
            fade_in: 5,
            fade_out: 7,
        }];
        let input = HirariRegionSplitInput {
            state: state(),
            split_offset: 400,
            markers: std::ptr::null(),
            marker_count: 0,
            ranges: ranges.as_ptr(),
            range_count: ranges.len(),
            comp_ranges: comps.as_ptr(),
            comp_range_count: comps.len(),
        };
        let mut out = plan(&input).expect("valid split");
        assert_eq!(out.left_range_count, 1);
        assert_eq!(out.right_range_count, 2);
        let left = unsafe { slice::from_raw_parts(out.left_ranges, out.left_range_count) };
        assert_eq!((left[0].start, left[0].end), (0, 400));
        assert_eq!((left[0].fade_in, left[0].fade_out), (10, 0));
        let right = unsafe { slice::from_raw_parts(out.right_ranges, out.right_range_count) };
        assert_eq!((right[0].start, right[0].end), (0, 100));
        assert_eq!((right[0].fade_in, right[0].fade_out), (0, 20));
        assert_eq!((right[1].start, right[1].end), (300, 500));
        let left_comps =
            unsafe { slice::from_raw_parts(out.left_comp_ranges, out.left_comp_range_count) };
        assert_eq!((left_comps[0].start, left_comps[0].end), (0, 400));
        assert_eq!((left_comps[0].fade_in, left_comps[0].fade_out), (5, 0));
        let right_comps =
            unsafe { slice::from_raw_parts(out.right_comp_ranges, out.right_comp_range_count) };
        assert_eq!((right_comps[0].fade_in, right_comps[0].fade_out), (0, 7));
        release(&mut out);
    }

    #[test]
    fn split_rejects_geometry_the_old_cpp_rejected() {
        let mut input = HirariRegionSplitInput {
            state: state(),
            split_offset: 0,
            markers: std::ptr::null(),
            marker_count: 0,
            ranges: std::ptr::null(),
            range_count: 0,
            comp_ranges: std::ptr::null(),
            comp_range_count: 0,
        };
        assert!(plan(&input).is_none(), "zero offset");
        input.split_offset = 1_000;
        assert!(plan(&input).is_none(), "offset at the region end");
        input.split_offset = 500;
        input.state.source_length = 1;
        assert!(plan(&input).is_none(), "single source sample");
        input.state = state();
        input.state.start = u64::MAX - 10;
        assert!(plan(&input).is_none(), "start + len overflow");
    }

    #[test]
    fn duplicate_trims_the_base_start_by_the_source_trim() {
        let mut source = state();
        source.start = 5_000;
        source.base_start = 5_000;
        source.source_offset = 20;
        source.base_source_offset = 10;
        let mut out = HirariRegionState::default();
        assert!(unsafe { hirari_region_duplicate_state(&source, 8_000, &mut out) });
        assert_eq!(out.start, 8_000);
        assert_eq!(out.base_start, 7_990);
        assert_eq!(out.len, source.len);

        assert!(!unsafe { hirari_region_duplicate_state(&source, 5, &mut out) });
        source.len = 0;
        assert!(!unsafe { hirari_region_duplicate_state(&source, 8_000, &mut out) });
    }

    fn split_store(
        store: *mut HirariRegionVariAudioStore,
        split_offset: u64,
        region_length: u64,
    ) -> (
        *mut HirariRegionVariAudioStore,
        *mut HirariRegionVariAudioStore,
    ) {
        let left = unsafe { hirari_region_vari_audio_store_new() };
        let right = unsafe { hirari_region_vari_audio_store_new() };
        assert!(unsafe {
            hirari_region_vari_audio_store_split(
                store,
                left,
                right,
                1_000.0,
                split_offset,
                region_length,
            )
        });
        (left, right)
    }

    #[test]
    fn note_store_split_windows_segments_and_rebuilds_boundary_anchors() {
        let store = unsafe { hirari_region_vari_audio_store_new() };
        let segment = HirariAudioNoteSegment {
            start_seconds: 0.0,
            end_seconds: 2.0,
            detected_pitch_cents: 4_400.0,
            pitch_offset_cents: 100.0,
            formant_offset_cents: -50.0,
        };
        assert!(unsafe { hirari_region_vari_audio_store_upsert(store, segment, std::ptr::null(), 0) });
        assert!(unsafe { hirari_region_vari_audio_store_upsert_anchor(store, 0.0, 0.5, 200.0, 50.0) });
        assert!(unsafe { hirari_region_vari_audio_store_upsert_anchor(store, 0.0, 1.5, 300.0, 80.0) });

        let (left, right) = split_store(store, 1_000, 2_000);
        let left_view = unsafe { crate::vari_audio::vari_store_view(left, 0.0) }.expect("left segment");
        assert_eq!((left_view.start, left_view.end), (0.0, 1.0));
        assert_eq!(left_view.detected_pitch, 4_400.0);
        assert_eq!((left_view.pitch_offset, left_view.formant_offset), (100.0, -50.0));
        assert_eq!(left_view.anchors.len(), 3);
        assert_eq!(left_view.anchors[0].position_seconds, 0.0);
        assert_eq!(left_view.anchors[1].position_seconds, 0.5);
        assert_eq!(left_view.anchors[2].position_seconds, 1.0);
        assert_eq!(left_view.anchors[1].pitch_cents, 200.0);
        assert_eq!(left_view.anchors[1].formant_cents, 50.0);
        // Boundary anchors store the curve component: the source curve at
        // 1.0 s is 250 cents (anchors 200 @ 0.5, 300 @ 1.5) and 65 formant
        // cents, both independent of the segment's own offsets.
        assert_eq!(left_view.anchors[2].pitch_cents, 250.0);
        assert_eq!(left_view.anchors[2].formant_cents, 65.0);

        let right_view = unsafe { crate::vari_audio::vari_store_view(right, 0.0) }.expect("right segment");
        assert_eq!((right_view.start, right_view.end), (0.0, 1.0));
        assert_eq!(right_view.anchors.len(), 3);
        assert_eq!(right_view.anchors[0].position_seconds, 0.0);
        assert_eq!(right_view.anchors[1].position_seconds, 0.5);
        assert_eq!(right_view.anchors[2].position_seconds, 1.0);
        assert_eq!(right_view.anchors[0].pitch_cents, 250.0);
        assert_eq!(right_view.anchors[0].formant_cents, 65.0);
        assert_eq!(right_view.anchors[1].pitch_cents, 300.0);
        assert_eq!(right_view.anchors[1].formant_cents, 80.0);
        // Past the last anchor the curve clamps, so the right edge is the
        // same curve component as the interior anchor.
        assert_eq!(right_view.anchors[2].pitch_cents, 300.0);
        assert_eq!(right_view.anchors[2].formant_cents, 80.0);

        unsafe {
            hirari_region_vari_audio_store_clear(left);
            hirari_region_vari_audio_store_clear(right);
            hirari_region_vari_audio_store_clear(store);
        }
    }

    #[test]
    fn note_store_split_drops_segments_outside_the_windows() {
        let store = unsafe { hirari_region_vari_audio_store_new() };
        for (start, end) in [(0.0, 0.5), (1.5, 2.0)] {
            let segment = HirariAudioNoteSegment {
                start_seconds: start,
                end_seconds: end,
                detected_pitch_cents: 0.0,
                pitch_offset_cents: 0.0,
                formant_offset_cents: 0.0,
            };
            assert!(unsafe { hirari_region_vari_audio_store_upsert(store, segment, std::ptr::null(), 0) });
        }
        let (left, right) = split_store(store, 1_000, 2_000);
        assert_eq!(unsafe { crate::vari_audio::vari_store_view(left, 0.0) }.unwrap().end, 0.5);
        assert!(unsafe { crate::vari_audio::vari_store_view(left, 1.0) }.is_none());
        assert!(unsafe { crate::vari_audio::vari_store_view(right, 0.0) }.is_none());
        assert_eq!(unsafe { crate::vari_audio::vari_store_view(right, 0.5) }.unwrap().end, 1.0);
        unsafe {
            hirari_region_vari_audio_store_clear(left);
            hirari_region_vari_audio_store_clear(right);
            hirari_region_vari_audio_store_clear(store);
        }
    }

    #[test]
    fn frozen_split_matches_the_store_split() {
        let store = unsafe { hirari_region_vari_audio_store_new() };
        let segment = HirariAudioNoteSegment {
            start_seconds: 0.0,
            end_seconds: 2.0,
            detected_pitch_cents: 4_400.0,
            pitch_offset_cents: 100.0,
            formant_offset_cents: -50.0,
        };
        assert!(unsafe { hirari_region_vari_audio_store_upsert(store, segment, std::ptr::null(), 0) });
        assert!(unsafe {
            hirari_region_vari_audio_store_upsert_anchor(store, 0.0, 0.5, 200.0, 50.0)
        });
        assert!(unsafe {
            hirari_region_vari_audio_store_upsert_anchor(store, 0.0, 1.5, 300.0, 80.0)
        });
        let (store_left, store_right) = split_store(store, 1_000, 2_000);

        let frozen = unsafe { hirari_region_vari_audio_freeze(store) };
        assert!(!frozen.is_null());
        let frozen_left = unsafe { hirari_region_vari_audio_store_new() };
        let frozen_right = unsafe { hirari_region_vari_audio_store_new() };
        assert!(unsafe {
            hirari_region_vari_audio_frozen_split(
                frozen,
                frozen_left,
                frozen_right,
                1_000.0,
                1_000,
                2_000,
            )
        });
        unsafe { hirari_region_vari_audio_frozen_free(frozen) };

        for (from_store, from_frozen) in [(store_left, frozen_left), (store_right, frozen_right)] {
            let a = unsafe { crate::vari_audio::vari_store_view(from_store, 0.0) }
                .expect("store split segment");
            let b = unsafe { crate::vari_audio::vari_store_view(from_frozen, 0.0) }
                .expect("frozen split segment");
            assert_eq!(
                (a.start, a.end, a.detected_pitch, a.pitch_offset, a.formant_offset),
                (b.start, b.end, b.detected_pitch, b.pitch_offset, b.formant_offset)
            );
            assert_eq!(a.anchors.len(), b.anchors.len());
            for (left, right) in a.anchors.iter().zip(b.anchors.iter()) {
                assert_eq!(
                    (
                        left.position_seconds,
                        left.pitch_cents,
                        left.formant_cents
                    ),
                    (
                        right.position_seconds,
                        right.pitch_cents,
                        right.formant_cents
                    )
                );
            }
            unsafe {
                hirari_region_vari_audio_store_clear(from_store);
                hirari_region_vari_audio_store_clear(from_frozen);
            }
        }
        unsafe { hirari_region_vari_audio_store_clear(store) };
    }
}
