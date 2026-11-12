//! Tests split out of region_jobs.rs.

    use super::*;

    #[test]
    fn region_key_packs_track_and_region() {
        assert_eq!(region_key(0, 0), 0);
        assert_eq!(region_key(1, 2), (1 << 32) | 2);
        assert_ne!(region_key(2, 1), region_key(1, 2));
    }

    #[test]
    fn sample_rate_bounds_match_native_import() {
        assert!(!valid_sample_rate(f64::NAN));
        assert!(!valid_sample_rate(f64::INFINITY));
        assert!(!valid_sample_rate(7_999.0));
        assert!(valid_sample_rate(8_000.0));
        assert!(valid_sample_rate(384_000.0));
        assert!(!valid_sample_rate(384_001.0));
    }
