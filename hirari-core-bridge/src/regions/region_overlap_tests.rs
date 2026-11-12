//! Tests split out of region_processing.rs.

    use super::{hirari_regions_apply_overlap_crossfades, HirariRegionOverlapView};

    #[test]
    fn derives_overlapping_fades_and_preserves_larger_existing_values() {
        let mut regions = [
            HirariRegionOverlapView {
                start: 100,
                length: 100,
                loop_count: 1,
                ..Default::default()
            },
            HirariRegionOverlapView {
                start: 150,
                length: 80,
                loop_count: 1,
                crossfade_in: 64,
                ..Default::default()
            },
            HirariRegionOverlapView {
                start: 90,
                length: 500,
                loop_count: 1,
                muted: 1,
                ..Default::default()
            },
        ];
        assert!(unsafe {
            hirari_regions_apply_overlap_crossfades(regions.as_mut_ptr(), regions.len())
        });
        assert_eq!(regions[0].crossfade_out, 50);
        assert_eq!(regions[1].crossfade_in, 64);
        assert_eq!(regions[2].crossfade_in, 0);
    }

    #[test]
    fn caps_long_loop_overlaps_and_saturates_timeline_end_overflow() {
        let mut regions = [
            HirariRegionOverlapView {
                start: 0,
                length: u64::MAX,
                loop_count: 3,
                ..Default::default()
            },
            HirariRegionOverlapView {
                start: 10,
                length: 2,
                loop_count: 1,
                ..Default::default()
            },
        ];
        assert!(unsafe {
            hirari_regions_apply_overlap_crossfades(regions.as_mut_ptr(), regions.len())
        });
        assert_eq!(regions[1].crossfade_in, 8192);
    }

    #[test]
    fn empty_input_is_valid_and_nonempty_null_input_is_rejected() {
        assert!(unsafe { hirari_regions_apply_overlap_crossfades(std::ptr::null_mut(), 0) });
        assert!(!unsafe { hirari_regions_apply_overlap_crossfades(std::ptr::null_mut(), 1) });
    }
