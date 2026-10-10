
/// C ABI mirror of a comping range. Layout matches `HirariRegionCompRange`
/// (and therefore `Region::CompRange`); see the static_assert at the C++ call
/// site.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeCompRange {
    pub start: u64,
    pub end: u64,
    pub fade_in: u64,
    pub fade_out: u64,
}

/// Decides whether a comp-range batch may be stored on a region.
///
/// This is the single owner of the rules that used to live in three places:
/// the C++ `Track::setRegionCompRanges` loop, the engine wrapper, and a
/// partial copy in the Rust arrangement layer that was missing the ordering
/// and region-bounds checks entirely.
///
/// The rules, in the order they reject:
/// * more than 4096 ranges
/// * changing the managed flag on a locked region (comp audio is what a lock
///   protects; the flag itself still moves)
/// * a null range pointer with a non-zero count
/// * per range: non-empty, fade-in and fade-out inside its own span, not
///   starting before the previous range ended (which also enforces ordering)
/// * every range inside the region length
///
/// An empty batch is admissible, so un-managing a region always works.
///
/// # Safety
/// When `range_count > 0`, `ranges` must be readable for
/// `range_count * size_of::<NativeCompRange>()` bytes. The buffer is only
/// borrowed for the duration of the call; nothing is retained.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_comp_ranges_validate(
    ranges: *const NativeCompRange,
    range_count: usize,
    region_len: u64,
    locked: bool,
    managed: bool,
) -> bool {
    if range_count > 4096 || (locked && managed) {
        return false;
    }
    if range_count == 0 {
        return true;
    }
    if ranges.is_null() {
        return false;
    }
    let ranges = unsafe { std::slice::from_raw_parts(ranges, range_count) };
    let mut previous_end = 0u64;
    for range in ranges {
        // `start >= end` rejects first, so the span subtraction below cannot
        // underflow the way an unsigned `end - start` would.
        if range.start >= range.end
            || range.fade_in > range.end - range.start
            || range.fade_out > range.end - range.start
            || range.start < previous_end
            || range.end > region_len
        {
            return false;
        }
        previous_end = range.end;
    }
    true
}

#[cfg(test)]
mod comp_range_policy_tests {
    use super::*;

    fn range(start: u64, end: u64, fade_in: u64, fade_out: u64) -> NativeCompRange {
        NativeCompRange {
            start,
            end,
            fade_in,
            fade_out,
        }
    }

    fn validate(
        ranges: &[NativeCompRange],
        region_len: u64,
        locked: bool,
        managed: bool,
    ) -> bool {
        unsafe {
            hirari_region_comp_ranges_validate(
                ranges.as_ptr(),
                ranges.len(),
                region_len,
                locked,
                managed,
            )
        }
    }

    #[test]
    fn accepts_ordered_non_overlapping_ranges_inside_the_region() {
        let ranges = [
            range(0, 100, 10, 10),
            range(100, 250, 0, 25),
            range(250, 400, 5, 5),
        ];
        assert!(validate(&ranges, 400, false, true));
        assert!(validate(&ranges, 500, false, true));
    }

    #[test]
    fn rejects_empty_inverted_and_overlapping_ranges() {
        // Empty.
        assert!(!validate(&[range(10, 10, 0, 0)], 100, false, true));
        // Inverted.
        assert!(!validate(&[range(50, 20, 0, 0)], 100, false, true));
        // Overlapping.
        assert!(!validate(
            &[range(0, 60, 0, 0), range(50, 90, 0, 0)],
            100,
            false,
            true
        ));
        // Out of order but still non-overlapping is rejected: the batch has to
        // arrive ordered.
        assert!(!validate(
            &[range(50, 90, 0, 0), range(0, 40, 0, 0)],
            100,
            false,
            true
        ));
        // Touching ranges are fine; only a strict overlap is refused.
        assert!(validate(
            &[range(0, 40, 0, 0), range(40, 90, 0, 0)],
            100,
            false,
            true
        ));
    }

    #[test]
    fn rejects_fades_wider_than_their_own_range() {
        assert!(!validate(&[range(0, 10, 11, 0)], 100, false, true));
        assert!(!validate(&[range(0, 10, 0, 11)], 100, false, true));
        // A fade exactly as wide as the range is the boundary case.
        assert!(validate(&[range(0, 10, 10, 10)], 100, false, true));
    }

    #[test]
    fn rejects_ranges_outside_the_region() {
        assert!(!validate(&[range(0, 101, 0, 0)], 100, false, true));
        // The earlier ranges stay valid; only the offending one is refused.
        assert!(!validate(
            &[range(0, 50, 0, 0), range(50, 101, 0, 0)],
            100,
            false,
            true
        ));
    }

    #[test]
    fn lock_only_blocks_the_managed_flag() {
        let ranges = [range(0, 50, 0, 0)];
        // Locked: flipping the flag on is refused.
        assert!(!validate(&ranges, 100, true, true));
        // Locked: keeping the flag off still lets the ranges move.
        assert!(validate(&ranges, 100, true, false));
        // Unlocked: both directions work.
        assert!(validate(&ranges, 100, false, true));
    }

    #[test]
    fn empty_batch_is_always_admissible_except_on_a_locked_region() {
        assert!(validate(&[], 100, false, true));
        assert!(validate(&[], 100, true, false));
        assert!(!validate(&[], 100, true, true));
    }

    #[test]
    fn rejects_oversize_batches_and_null_pointers() {
        let many: Vec<NativeCompRange> = (0..4097)
            .map(|index| range(index, index + 1, 0, 0))
            .collect();
        assert!(!validate(&many, u64::MAX, false, false));
        // 4096 is the boundary and stays admissible when ordered and inside.
        let boundary: Vec<NativeCompRange> = (0..4096)
            .map(|index| range(index * 2, index * 2 + 1, 0, 0))
            .collect();
        assert!(validate(&boundary, u64::MAX, false, false));
        // A non-zero count with a null pointer is refused rather than read.
        assert!(!unsafe {
            hirari_region_comp_ranges_validate(std::ptr::null(), 4, 100, false, true)
        });
    }
}
