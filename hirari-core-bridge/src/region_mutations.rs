use std::slice;

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
