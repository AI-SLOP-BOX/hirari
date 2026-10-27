use std::slice;

use crate::region_warp::{hirari_region_source_position_at, HirariWarpMarker};

/// Splits a piecewise-linear source/timeline warp map at a region-relative
/// timeline sample.
pub(crate) fn split_warp_markers(
    input: &[HirariWarpMarker],
    source_span: u64,
    timeline_length: u64,
    split_offset: u64,
    source_frames_per_timeline_frame: f64,
) -> Option<(Vec<HirariWarpMarker>, Vec<HirariWarpMarker>, u64)> {
    if timeline_length == 0
        || split_offset == 0
        || split_offset >= timeline_length
        || source_span <= 1
        || input.len() > 100_000
    {
        return None;
    }
    let source_at = |sample| {
        if input.len() < 2 {
            (sample as f64 * source_frames_per_timeline_frame).clamp(0.0, source_span as f64)
        } else {
            unsafe {
                hirari_region_source_position_at(
                    input.as_ptr(),
                    input.len(),
                    sample,
                    1.0,
                    source_span,
                )
            }
        }
    };
    let mapped_split = source_at(split_offset);
    let mapped_end = source_at(timeline_length);
    let mapped_start = source_at(0);
    if !mapped_split.is_finite() || !mapped_end.is_finite() || !mapped_start.is_finite() {
        return None;
    }
    let source_split = mapped_split.round().clamp(1.0, (source_span - 1) as f64) as u64;
    let right_source_length = source_span - source_split;
    let left_start = HirariWarpMarker {
        source_sample: mapped_start.round().clamp(0.0, source_split as f64) as u64,
        timeline_sample: 0,
        transient: 0,
    };
    let left_boundary = HirariWarpMarker {
        source_sample: source_split,
        timeline_sample: split_offset,
        transient: 0,
    };
    let right_boundary = HirariWarpMarker {
        source_sample: 0,
        timeline_sample: 0,
        transient: 0,
    };
    let right_end_source =
        (mapped_end.round() - source_split as f64).clamp(0.0, right_source_length as f64) as u64;
    let mut left = Vec::with_capacity(input.len() + 2);
    let mut right = Vec::with_capacity(input.len() + 2);
    if input.len() >= 2 {
        for marker in input {
            if marker.timeline_sample < split_offset
                && marker.source_sample > left_start.source_sample
                && marker.source_sample < source_split
            {
                left.push(*marker);
            } else if marker.timeline_sample == split_offset {
                left.push(left_boundary);
            }
            if marker.timeline_sample > split_offset
                && marker.source_sample > source_split
                && marker.source_sample - source_split < right_end_source
            {
                right.push(HirariWarpMarker {
                    source_sample: marker.source_sample - source_split,
                    timeline_sample: marker.timeline_sample - split_offset,
                    transient: marker.transient,
                });
            }
        }
        if left
            .last()
            .map_or(true, |marker| marker.timeline_sample != split_offset)
        {
            left.push(left_boundary);
        } else if let Some(last) = left.last_mut() {
            *last = left_boundary;
        }
        if right
            .first()
            .map_or(true, |marker| marker.timeline_sample != 0)
        {
            right.insert(0, right_boundary);
        } else if let Some(first) = right.first_mut() {
            *first = right_boundary;
        }
        if left
            .first()
            .map_or(true, |marker| marker.timeline_sample != 0)
        {
            left.insert(0, left_start);
        } else if let Some(first) = left.first_mut() {
            *first = left_start;
        }
        let right_end = HirariWarpMarker {
            source_sample: right_end_source,
            timeline_sample: timeline_length - split_offset,
            transient: 0,
        };
        if right.last().map_or(true, |marker| {
            marker.timeline_sample != right_end.timeline_sample
        }) {
            right.push(right_end);
        } else if let Some(last) = right.last_mut() {
            *last = right_end;
        }
    }
    Some((left, right, source_split))
}

/// Applies a synchronized timeline delta to a snapshot of region and base
/// positions. All overflow checks complete before outputs are committed.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_sync_move_positions(
    starts: *const u64,
    base_starts: *const u64,
    count: usize,
    anchor_start: u64,
    new_anchor_start: u64,
    output_starts: *mut u64,
    output_base_starts: *mut u64,
) -> bool {
    if count == 0
        || starts.is_null()
        || base_starts.is_null()
        || output_starts.is_null()
        || output_base_starts.is_null()
    {
        return false;
    }
    let starts = slice::from_raw_parts(starts, count);
    let base_starts = slice::from_raw_parts(base_starts, count);
    let mut moved_starts = Vec::with_capacity(count);
    let mut moved_base_starts = Vec::with_capacity(count);
    if new_anchor_start >= anchor_start {
        let offset = new_anchor_start - anchor_start;
        for (&start, &base_start) in starts.iter().zip(base_starts) {
            let (Some(start), Some(base_start)) =
                (start.checked_add(offset), base_start.checked_add(offset))
            else {
                return false;
            };
            moved_starts.push(start);
            moved_base_starts.push(base_start);
        }
    } else {
        let offset = anchor_start - new_anchor_start;
        for (&start, &base_start) in starts.iter().zip(base_starts) {
            let (Some(start), Some(base_start)) =
                (start.checked_sub(offset), base_start.checked_sub(offset))
            else {
                return false;
            };
            moved_starts.push(start);
            moved_base_starts.push(base_start);
        }
    }
    slice::from_raw_parts_mut(output_starts, count).copy_from_slice(&moved_starts);
    slice::from_raw_parts_mut(output_base_starts, count).copy_from_slice(&moved_base_starts);
    true
}

#[cfg(test)]
mod tests {
    use super::hirari_region_sync_move_positions;

    #[test]
    fn sync_move_applies_one_delta_to_every_position() {
        let starts = [100, 350, 900];
        let bases = [80, 300, 800];
        let mut moved = [0; 3];
        let mut moved_bases = [0; 3];
        assert!(unsafe {
            hirari_region_sync_move_positions(
                starts.as_ptr(),
                bases.as_ptr(),
                3,
                100,
                125,
                moved.as_mut_ptr(),
                moved_bases.as_mut_ptr(),
            )
        });
        assert_eq!(moved, [125, 375, 925]);
        assert_eq!(moved_bases, [105, 325, 825]);
    }

    #[test]
    fn sync_move_rejects_underflow_and_overflow_without_partial_output() {
        let starts = [100, 5];
        let bases = [80, 4];
        let mut moved = [77; 2];
        let mut moved_bases = [88; 2];
        assert!(!unsafe {
            hirari_region_sync_move_positions(
                starts.as_ptr(),
                bases.as_ptr(),
                2,
                100,
                90,
                moved.as_mut_ptr(),
                moved_bases.as_mut_ptr(),
            )
        });
        assert_eq!(moved, [77; 2]);
        assert_eq!(moved_bases, [88; 2]);

        let starts = [u64::MAX - 1];
        let bases = [1];
        let mut moved = [77];
        let mut moved_bases = [88];
        assert!(!unsafe {
            hirari_region_sync_move_positions(
                starts.as_ptr(),
                bases.as_ptr(),
                1,
                0,
                2,
                moved.as_mut_ptr(),
                moved_bases.as_mut_ptr(),
            )
        });
        assert_eq!(moved, [77]);
        assert_eq!(moved_bases, [88]);
    }
}
