//! Tests split out of audio_note_curve.rs.

    use super::{
        hirari_audio_note_can_warp_segment, hirari_audio_note_curve_component_anchor,
        hirari_audio_note_curve_move_anchor, hirari_audio_note_curve_upsert_anchor,
        hirari_audio_note_curve_warp_anchors, hirari_audio_note_find_segment_for_edit,
        hirari_audio_note_rebuild_phase_prefixes, hirari_audio_note_upsert_segment_slot,
        hirari_audio_note_validate_segments, HirariAudioNoteAnchor, HirariAudioNotePhaseView,
        HirariAudioNoteSegmentValidation,
    };

    #[test]
    fn segment_batch_validation_sorts_and_rejects_invalid_batches() {
        let anchors = [HirariAudioNoteAnchor {
            position_seconds: 1.0,
            pitch_cents: 0.0,
            formant_cents: 0.0,
        }];
        let segments = [
            HirariAudioNoteSegmentValidation {
                start_seconds: 4.0,
                end_seconds: 6.0,
                detected_pitch_cents: 100.0,
                ..Default::default()
            },
            HirariAudioNoteSegmentValidation {
                start_seconds: 0.0,
                end_seconds: 2.0,
                detected_pitch_cents: 90.0,
                anchors: anchors.as_ptr(),
                anchor_count: anchors.len(),
                ..Default::default()
            },
        ];
        let mut order = [u32::MAX; 2];
        assert!(unsafe {
            hirari_audio_note_validate_segments(
                segments.as_ptr(),
                segments.len(),
                order.as_mut_ptr(),
            )
        });
        assert_eq!(order, [1, 0]);

        let overlapping = [
            segments[1],
            HirariAudioNoteSegmentValidation {
                start_seconds: 1.5,
                end_seconds: 3.0,
                ..segments[1]
            },
        ];
        assert!(!unsafe {
            hirari_audio_note_validate_segments(
                overlapping.as_ptr(),
                overlapping.len(),
                order.as_mut_ptr(),
            )
        });

        let invalid_anchor = [HirariAudioNoteAnchor {
            position_seconds: 2.5,
            ..anchors[0]
        }];
        let invalid = [HirariAudioNoteSegmentValidation {
            anchors: invalid_anchor.as_ptr(),
            ..segments[1]
        }];
        assert!(!unsafe {
            hirari_audio_note_validate_segments(invalid.as_ptr(), invalid.len(), order.as_mut_ptr())
        });
    }

    #[test]
    fn phase_prefix_rebuild_sorts_and_accumulates_pitch_ratio_delta() {
        let segments = [
            HirariAudioNotePhaseView {
                start_seconds: 3.0,
                end_seconds: 4.0,
                ..Default::default()
            },
            HirariAudioNotePhaseView {
                start_seconds: 0.0,
                end_seconds: 1.0,
                pitch_offset_cents: 1200.0,
                ..Default::default()
            },
        ];
        let mut order = [u32::MAX; 2];
        let mut corrections = [f64::NAN; 2];
        assert!(unsafe {
            hirari_audio_note_rebuild_phase_prefixes(
                segments.as_ptr(),
                segments.len(),
                order.as_mut_ptr(),
                corrections.as_mut_ptr(),
            )
        });
        assert_eq!(order, [1, 0]);
        assert_eq!(corrections, [0.0, 1.0]);
    }

    #[test]
    fn segment_edit_rules_match_existing_overlap_and_start_tolerance() {
        let ranges = [
            super::HirariAudioNoteSegmentRange {
                start_seconds: 1.0,
                end_seconds: 2.0,
                detected_pitch_cents: 0.0,
            },
            super::HirariAudioNoteSegmentRange {
                start_seconds: 3.0,
                end_seconds: 4.0,
                detected_pitch_cents: 0.0,
            },
        ];
        assert_eq!(
            unsafe { hirari_audio_note_upsert_segment_slot(ranges.as_ptr(), 2, 1.0, 1.5) },
            0
        );
        assert_eq!(
            unsafe { hirari_audio_note_upsert_segment_slot(ranges.as_ptr(), 2, 1.5, 2.5) },
            -2
        );
        assert_eq!(
            unsafe { hirari_audio_note_upsert_segment_slot(ranges.as_ptr(), 2, 2.0, 3.0) },
            -1
        );
        assert_eq!(
            unsafe { hirari_audio_note_find_segment_for_edit(ranges.as_ptr(), 2, 1.0 + 5.0e-10) },
            0
        );
        assert!(unsafe { hirari_audio_note_can_warp_segment(ranges.as_ptr(), 2, 0, 0.5, 2.5) });
        assert!(!unsafe { hirari_audio_note_can_warp_segment(ranges.as_ptr(), 2, 0, 1.5, 3.5) });
    }

    #[test]
    fn anchor_upsert_inserts_in_order_and_replaces_nearby_points() {
        let mut anchors = [HirariAudioNoteAnchor::default(); 4];
        anchors[0] = HirariAudioNoteAnchor {
            position_seconds: 1.0,
            pitch_cents: 10.0,
            formant_cents: 20.0,
        };
        anchors[1] = HirariAudioNoteAnchor {
            position_seconds: 3.0,
            pitch_cents: 30.0,
            formant_cents: 40.0,
        };
        let count = unsafe {
            hirari_audio_note_curve_upsert_anchor(
                anchors.as_mut_ptr(),
                2,
                4,
                0.0,
                4.0,
                2.0,
                50.0,
                60.0,
            )
        };
        assert_eq!(count, 3);
        assert_eq!(
            anchors[..count]
                .iter()
                .map(|a| a.position_seconds)
                .collect::<Vec<_>>(),
            [1.0, 2.0, 3.0]
        );

        let count = unsafe {
            hirari_audio_note_curve_upsert_anchor(
                anchors.as_mut_ptr(),
                count,
                4,
                0.0,
                4.0,
                2.0 - 5.0e-10,
                70.0,
                80.0,
            )
        };
        assert_eq!(count, 3);
        assert_eq!(anchors[1].pitch_cents, 70.0);
        assert_eq!(anchors[1].formant_cents, 80.0);
    }

    #[test]
    fn anchor_warp_preserves_relative_positions_and_rejects_invalid_ranges() {
        let mut anchors = [
            HirariAudioNoteAnchor {
                position_seconds: 2.0,
                pitch_cents: 1.0,
                formant_cents: 2.0,
            },
            HirariAudioNoteAnchor {
                position_seconds: 4.0,
                pitch_cents: 3.0,
                formant_cents: 4.0,
            },
        ];
        assert!(unsafe {
            hirari_audio_note_curve_warp_anchors(anchors.as_mut_ptr(), 2, 1.0, 5.0, 10.0, 16.0)
        });
        assert_eq!(anchors[0].position_seconds, 11.5);
        assert_eq!(anchors[1].position_seconds, 14.5);
        assert!(!unsafe {
            hirari_audio_note_curve_warp_anchors(anchors.as_mut_ptr(), 2, 1.0, 1.0, 0.0, 2.0)
        });
        assert_eq!(anchors[0].position_seconds, 11.5);
    }

    #[test]
    fn moving_an_anchor_reorders_or_merges_and_keeps_failure_atomic() {
        let mut anchors = [
            HirariAudioNoteAnchor {
                position_seconds: 1.0,
                pitch_cents: 10.0,
                formant_cents: 11.0,
            },
            HirariAudioNoteAnchor {
                position_seconds: 3.0,
                pitch_cents: 30.0,
                formant_cents: 31.0,
            },
        ];
        let count = unsafe {
            hirari_audio_note_curve_move_anchor(
                anchors.as_mut_ptr(),
                2,
                0.0,
                4.0,
                1.0,
                2.0,
                150.0,
                0,
                25.0,
                5.0,
            )
        };
        assert_eq!(count, 2);
        assert_eq!(anchors[0].position_seconds, 2.0);
        assert_eq!(anchors[0].pitch_cents, 125.0);

        let count = unsafe {
            hirari_audio_note_curve_move_anchor(
                anchors.as_mut_ptr(),
                2,
                0.0,
                4.0,
                2.0,
                3.0,
                240.0,
                1,
                25.0,
                40.0,
            )
        };
        assert_eq!(count, 1);
        assert_eq!(anchors[0].position_seconds, 3.0);
        assert_eq!(anchors[0].formant_cents, 200.0);
        let before = anchors[0];
        let failed = unsafe {
            hirari_audio_note_curve_move_anchor(
                anchors.as_mut_ptr(),
                1,
                0.0,
                4.0,
                1.5,
                2.0,
                100.0,
                0,
                0.0,
                0.0,
            )
        };
        assert_eq!(failed, usize::MAX);
        assert_eq!(anchors[0].position_seconds, before.position_seconds);
        assert_eq!(anchors[0].pitch_cents, before.pitch_cents);
        assert_eq!(anchors[0].formant_cents, before.formant_cents);
    }

    #[test]
    fn component_edit_preserves_the_other_interpolated_curve() {
        let anchors = [
            HirariAudioNoteAnchor {
                position_seconds: 1.0,
                pitch_cents: 100.0,
                formant_cents: -20.0,
            },
            HirariAudioNoteAnchor {
                position_seconds: 3.0,
                pitch_cents: 300.0,
                formant_cents: 20.0,
            },
        ];
        let mut output = HirariAudioNoteAnchor::default();
        assert!(unsafe {
            hirari_audio_note_curve_component_anchor(
                anchors.as_ptr(),
                2,
                0.0,
                4.0,
                2.0,
                500.0,
                0,
                10.0,
                5.0,
                &mut output,
            )
        });
        assert_eq!(output.pitch_cents, 490.0);
        assert_eq!(output.formant_cents, 0.0);

        assert!(unsafe {
            hirari_audio_note_curve_component_anchor(
                anchors.as_ptr(),
                2,
                0.0,
                4.0,
                2.0,
                100.0,
                1,
                10.0,
                5.0,
                &mut output,
            )
        });
        assert_eq!(output.pitch_cents, 200.0);
        assert_eq!(output.formant_cents, 95.0);
    }
