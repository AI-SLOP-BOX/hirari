//! Tests split out of region_processing.rs.
use super::*;


    use HirariRegionScalarField as F;

    fn edit(field: HirariRegionScalarField, a: f32, b: f32) -> HirariRegionScalarEdit {
        HirariRegionScalarEdit {
            field,
            locked: false,
            value_a: a,
            value_b: b,
            region_len: 0,
            region_start: 0,
        }
    }

    fn decide(edit: HirariRegionScalarEdit) -> HirariRegionScalarDecision {
        HirariRegionScalarDecision::decide(&edit)
    }

    #[test]
    fn clip_gain_clamps_and_rejects_non_finite() {
        assert_eq!(decide(edit(F::ClipGain, 5.0, 0.0)).value_a, 2.0);
        assert!(decide(edit(F::ClipGain, 5.0, 0.0)).accepted);
        assert_eq!(decide(edit(F::ClipGain, -1.0, 0.0)).value_a, 0.0);
        assert!(decide(edit(F::ClipGain, -1.0, 0.0)).accepted);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(!decide(edit(F::ClipGain, bad, 0.0)).accepted, "{bad:?}");
        }
    }

    #[test]
    fn pitch_semitones_rejects_out_of_range_without_clamping() {
        for good in [-24.0, 0.0, 24.0] {
            assert!(decide(edit(F::PitchSemitones, good, 0.0)).accepted, "{good}");
        }
        for bad in [-24.001, 24.001, f32::NAN] {
            assert!(!decide(edit(F::PitchSemitones, bad, 0.0)).accepted, "{bad}");
        }
    }

    #[test]
    fn fades_clamp_both_ends_and_reject_non_finite_either_end() {
        let d = decide(edit(F::Fades, 2.0, -1.0));
        assert!(d.accepted);
        assert_eq!(d.value_a, 1.0);
        assert_eq!(d.value_b, 0.0);
        assert!(!decide(edit(F::Fades, f32::NAN, 0.5)).accepted);
        assert!(!decide(edit(F::Fades, 0.5, f32::NAN)).accepted);
    }

    fn loop_count_bits(count: u32) -> f32 {
        // LoopCount travels as a raw bit pattern, not a numeric value.
        f32::from_bits(count)
    }

    #[test]
    fn loop_count_bounds_and_overflow_guard() {
        assert!(decide(edit(F::LoopCount, loop_count_bits(1), 0.0)).accepted);
        assert!(decide(edit(F::LoopCount, loop_count_bits(1024), 0.0)).accepted);
        assert!(!decide(edit(F::LoopCount, loop_count_bits(0), 0.0)).accepted);
        assert!(!decide(edit(F::LoopCount, loop_count_bits(1025), 0.0)).accepted);

        // len * count must not overflow u64.
        let mut e = edit(F::LoopCount, loop_count_bits(4), 0.0);
        e.region_len = u64::MAX / 2;
        assert!(!decide(e).accepted);

        // start + len * count must not overflow u64.
        let mut e = edit(F::LoopCount, loop_count_bits(2), 0.0);
        e.region_len = u64::MAX / 2;
        e.region_start = u64::MAX / 2 + 1;
        assert!(!decide(e).accepted);

        let mut e = edit(F::LoopCount, loop_count_bits(2), 0.0);
        e.region_len = 1000;
        e.region_start = 10;
        assert!(decide(e).accepted);
    }

    #[test]
    fn locked_regions_reject_every_mutation() {
        for field in [
            F::ClipGain,
            F::Muted,
            F::Reverse,
            F::PitchSemitones,
            F::LoopCount,
            F::SyncGroup,
            F::Fades,
        ] {
            let mut e = edit(field, 1.0, 1.0);
            e.locked = true;
            assert!(!decide(e).accepted, "{field:?} must reject when locked");
        }
    }

    #[test]
    fn sync_group_passes_any_u32_tag_through_verbatim() {
        for tag in [0u32, 1, 7, 0xDEAD_BEEF, u32::MAX] {
            let d = decide(edit(F::SyncGroup, f32::from_bits(tag), 0.0));
            assert!(d.accepted, "tag {tag:#x}");
            assert_eq!(d.value_a.to_bits(), tag);
        }
    }

    #[test]
    fn fade_samples_truncates_like_the_previous_cpp_cast() {
        assert_eq!(unsafe { hirari_region_fade_samples(0.0, 1000) }, 0);
        assert_eq!(unsafe { hirari_region_fade_samples(-0.5, 1000) }, 0);
        assert_eq!(unsafe { hirari_region_fade_samples(0.5, 1000) }, 500);
        // Truncation, not rounding: 0.3333 * 1000 = 333.3 -> 333.
        assert_eq!(unsafe { hirari_region_fade_samples(0.3333, 1000) }, 333);
        // Clamped to the region length.
        assert_eq!(unsafe { hirari_region_fade_samples(2.0, 1000) }, 1000);
        // Zero-length regions produce zero samples rather than panicking.
        assert_eq!(unsafe { hirari_region_fade_samples(0.5, 0) }, 0);
        assert_eq!(unsafe { hirari_region_fade_samples(f32::NAN, 1000) }, 0);
    }

    #[test]
    fn c_abi_entry_point_fills_out_param_and_rejects_null() {
        let mut out = HirariRegionScalarDecision::default();
        let filled =
            unsafe { hirari_region_scalar_edit_decision(edit(F::ClipGain, 5.0, 0.0), &mut out) };
        assert!(filled);
        assert!(out.accepted);
        assert_eq!(out.value_a, 2.0);

        let filled =
            unsafe { hirari_region_scalar_edit_decision(edit(F::ClipGain, 1.0, 0.0), ptr::null_mut()) };
        assert!(!filled);
    }
