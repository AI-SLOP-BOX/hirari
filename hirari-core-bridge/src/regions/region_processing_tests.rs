//! Tests split out of region_processing.rs.

use super::*;

    #[test]
    fn region_range_edits_are_bounded_validated_and_sorted() {
        let edits = [
            NativeRangeEdit {
                start: 12,
                end: 40,
                gain: 2.0,
                fade_in: 99,
                fade_out: 2,
            },
            NativeRangeEdit {
                start: 0,
                end: 8,
                gain: 1.0,
                fade_in: 3,
                fade_out: 4,
            },
            NativeRangeEdit {
                start: 7,
                end: 70,
                gain: 0.0,
                fade_in: 50,
                fade_out: 50,
            },
            NativeRangeEdit {
                start: 5,
                end: 5,
                gain: 1.0,
                fade_in: 0,
                fade_out: 0,
            },
            NativeRangeEdit {
                start: 2,
                end: 9,
                gain: f32::NAN,
                fade_in: 0,
                fade_out: 0,
            },
        ];
        // The source list is left untouched: normalization is a pure transform
        // that returns an owned buffer rather than writing through a pointer.
        let mut count = 0usize;
        let normalized = unsafe {
            hirari_region_range_edits_apply(
                edits.as_ptr(),
                edits.len(),
                20,
                0,
                0,
                0,
                0.0,
                0,
                0,
                &mut count,
            )
        };
        assert!(!normalized.is_null());
        assert_eq!(count, 3);
        let result =
            unsafe { std::slice::from_raw_parts(normalized, count) }.to_vec();
        assert_eq!(result[0].start, 0);
        assert_eq!(result[0].fade_in, 3);
        assert_eq!(result[0].fade_out, 4);
        assert_eq!(result[1].start, 7);
        assert_eq!(result[1].end, 20);
        assert_eq!(result[1].fade_in, 13);
        assert_eq!(result[1].fade_out, 13);
        assert_eq!(result[2].start, 12);
        assert_eq!(result[2].end, 20);
        assert_eq!(result[2].fade_in, 8);
        assert_eq!(result[2].fade_out, 2);
        unsafe { hirari_free_range_edits(normalized, count) };
        assert_eq!(edits.len(), 5, "input must not be modified in place");
    }

    #[cfg(feature = "dsp-differential-reference")]
    use crate::audio_note_curve::{HirariAudioNoteAnchor, HirariAudioNoteCurveView};
    #[cfg(feature = "dsp-differential-reference")]
    use crate::region_warp::HirariWarpMarker;

    #[test]
    fn realtime_region_gain_fade_sanitizes_and_clamps_samples() {
        let mut samples = [f32::NAN, 1.0, -2.0, 0.5];
        assert!(unsafe {
            hirari_region_processor_apply_gain_fade(
                samples.as_mut_ptr(),
                samples.len() as u32,
                0,
                2.0,
                2,
                0,
            )
        });
        assert_eq!(samples, [0.0, 1.0, -4.0, 1.0]);
        assert!(!unsafe {
            hirari_region_processor_apply_gain_fade(
                samples.as_mut_ptr(),
                samples.len() as u32,
                0,
                1.0,
                2,
                9,
            )
        });
    }

    #[cfg(feature = "dsp-differential-reference")]
    unsafe extern "C" {
        fn hirari_region_gain_reference(
            output: *mut f32,
            frames: u32,
            region_offset: u64,
            region_length: u64,
            timeline_length: u64,
            fade_in: u64,
            fade_out: u64,
            crossfade_in: u64,
            crossfade_out: u64,
            clip_gain: f32,
            comp_managed: u8,
            comp_ranges: *const NativeCompRange,
            comp_range_count: usize,
            range_edits: *const NativeRangeEdit,
            range_edit_count: usize,
        );
        fn audio_note_curve_reference_evaluate(
            anchors: *const HirariAudioNoteAnchor,
            count: usize,
            prefix: *const f64,
            start: f64,
            finish: f64,
            pitch_offset: f64,
            formant_offset: f64,
            seconds: f64,
            output: *mut f64,
        );
        fn audio_note_segment_lookup_reference(
            ranges: *const HirariAudioNoteSegmentRange,
            count: usize,
            seconds: f64,
            output: *mut i64,
        );
        fn hirari_region_source_position_reference(
            markers: *const HirariWarpMarker,
            marker_count: usize,
            timeline_sample: u64,
            source_rate: f64,
            source_span: u64,
        ) -> f64;
    }

    #[test]
    #[cfg(feature = "dsp-differential-reference")]
    fn region_gain_block_matches_frozen_cpp_renderer() {
        let comp_ranges = [
            NativeCompRange {
                start: 0,
                end: 32,
                fade_in: 8,
                fade_out: 4,
            },
            NativeCompRange {
                start: 32,
                end: 64,
                fade_in: 4,
                fade_out: 12,
            },
        ];
        let range_edits = [
            NativeRangeEdit {
                start: 0,
                end: 20,
                gain: 0.75,
                fade_in: 6,
                fade_out: 5,
            },
            NativeRangeEdit {
                start: 16,
                end: 44,
                gain: 1.4,
                fade_in: 4,
                fade_out: 8,
            },
            NativeRangeEdit {
                start: 48,
                end: 64,
                gain: 0.0,
                fade_in: 0,
                fade_out: 0,
            },
        ];

        for comp_managed in [0, 1] {
            for offset in [0, 1, 12, 29, 31, 32, 47, 60, 91, 120, 151] {
                let mut rust = [0.0f32; 37];
                let mut cpp = [0.0f32; 37];
                unsafe {
                    hirari_region_gain_block(
                        rust.as_mut_ptr(),
                        rust.len() as u32,
                        offset,
                        64,
                        192,
                        10,
                        14,
                        7,
                        9,
                        1.25,
                        comp_managed,
                        comp_ranges.as_ptr(),
                        comp_ranges.len(),
                        range_edits.as_ptr(),
                        range_edits.len(),
                    );
                    hirari_region_gain_reference(
                        cpp.as_mut_ptr(),
                        cpp.len() as u32,
                        offset,
                        64,
                        192,
                        10,
                        14,
                        7,
                        9,
                        1.25,
                        comp_managed,
                        comp_ranges.as_ptr(),
                        comp_ranges.len(),
                        range_edits.as_ptr(),
                        range_edits.len(),
                    );
                }
                for (index, (rust, cpp)) in rust.iter().zip(cpp).enumerate() {
                    assert!(
                        (rust - cpp).abs() <= 2.0e-6,
                        "gain mismatch at offset={offset}, comp={comp_managed}, frame={index}: Rust={rust}, C++={cpp}"
                    );
                }
            }
        }
    }

    #[test]
    #[cfg(feature = "dsp-differential-reference")]
    fn rust_region_scheduler_block_matches_frozen_cpp_components() {
        let config = HirariRegionBlockConfig {
            region_offset: 5,
            region_length: 8,
            timeline_length: 16,
            fade_in: 3,
            fade_out: 4,
            crossfade_in: 2,
            crossfade_out: 3,
            clip_gain: 1.25,
            comp_managed: 1,
            sample_rate: 1_000.0,
            source_rate: 1.25,
            source_span: 16,
        };
        let comp_ranges = [
            NativeCompRange {
                start: 0,
                end: 4,
                fade_in: 2,
                fade_out: 1,
            },
            NativeCompRange {
                start: 4,
                end: 8,
                fade_in: 1,
                fade_out: 2,
            },
        ];
        let range_edits = [NativeRangeEdit {
            start: 1,
            end: 7,
            gain: 0.75,
            fade_in: 2,
            fade_out: 3,
        }];
        let note_ranges = [
            HirariAudioNoteSegmentRange {
                start_seconds: 0.0,
                end_seconds: 0.004,

                detected_pitch_cents: 0.0,
            },
            HirariAudioNoteSegmentRange {
                start_seconds: 0.002,
                end_seconds: 0.007,

                detected_pitch_cents: 0.0,
            },
        ];
        let anchors = [
            [
                HirariAudioNoteAnchor {
                    position_seconds: 0.0,
                    pitch_cents: -300.0,
                    formant_cents: 125.0,
                },
                HirariAudioNoteAnchor {
                    position_seconds: 0.004,
                    pitch_cents: 450.0,
                    formant_cents: -240.0,
                },
            ],
            [
                HirariAudioNoteAnchor {
                    position_seconds: 0.002,
                    pitch_cents: 175.0,
                    formant_cents: -75.0,
                },
                HirariAudioNoteAnchor {
                    position_seconds: 0.007,
                    pitch_cents: -625.0,
                    formant_cents: 350.0,
                },
            ],
        ];
        let pitch_offsets = [85.0, -40.0];
        let formant_offsets = [-30.0, 65.0];
        let mut prefixes = [[0.0; 2]; 2];
        let mut curves = Vec::new();
        for index in 0..2 {
            unsafe {
                crate::audio_note_curve::hirari_audio_note_curve_build_integral_prefix(
                    anchors[index].as_ptr(),
                    anchors[index].len(),
                    pitch_offsets[index],
                    prefixes[index].as_mut_ptr(),
                );
            }
            curves.push(HirariAudioNoteCurveView {
                anchors: anchors[index].as_ptr(),
                anchor_count: anchors[index].len(),
                integral_prefix: prefixes[index].as_ptr(),
                start_seconds: note_ranges[index].start_seconds,
                end_seconds: note_ranges[index].end_seconds,
                pitch_offset_cents: pitch_offsets[index],
                correction_before_seconds: 0.0,
                formant_offset_cents: formant_offsets[index],
            });
        }
        let warp_markers = [
            HirariWarpMarker {
                source_sample: 0,
                timeline_sample: 0,
                transient: 0,
            },
            HirariWarpMarker {
                source_sample: 12,
                timeline_sample: 8,
                transient: 1,
            },
        ];

        const FRAMES: usize = 13;
        let mut output_gain = [0.0f32; FRAMES];
        let mut output_pitch = [0.0f64; FRAMES];
        let mut output_formant = [0.0f64; FRAMES];
        let mut output_matched = [-1i64; FRAMES];
        let mut output_previous = [-1i64; FRAMES];
        let mut output_positions = [0.0f64; FRAMES];
        let mut output_rates = [0.0f64; FRAMES];
        let output = HirariRegionBlockOutput {
            gain: output_gain.as_mut_ptr(),
            pitch_cents: output_pitch.as_mut_ptr(),
            formant_cents: output_formant.as_mut_ptr(),
            matched_note_indices: output_matched.as_mut_ptr(),
            previous_note_indices: output_previous.as_mut_ptr(),
            source_positions: output_positions.as_mut_ptr(),
            local_source_rates: output_rates.as_mut_ptr(),
        };
        assert_eq!(std::mem::size_of::<HirariRegionBlockConfig>(), 88);
        assert_eq!(std::mem::size_of::<HirariRegionBlockOutput>(), 56);
        assert!(unsafe {
            hirari_region_prepare_block(
                &config,
                FRAMES as u32,
                comp_ranges.as_ptr(),
                comp_ranges.len(),
                range_edits.as_ptr(),
                range_edits.len(),
                note_ranges.as_ptr(),
                note_ranges.len(),
                curves.as_ptr(),
                curves.len(),
                warp_markers.as_ptr(),
                warp_markers.len(),
                &output,
            )
        });

        let mut cpp_gain = [0.0f32; FRAMES];
        unsafe {
            hirari_region_gain_reference(
                cpp_gain.as_mut_ptr(),
                FRAMES as u32,
                config.region_offset,
                config.region_length,
                config.timeline_length,
                config.fade_in,
                config.fade_out,
                config.crossfade_in,
                config.crossfade_out,
                config.clip_gain,
                config.comp_managed,
                comp_ranges.as_ptr(),
                comp_ranges.len(),
                range_edits.as_ptr(),
                range_edits.len(),
            );
        }
        for frame in 0..FRAMES {
            assert!((output_gain[frame] - cpp_gain[frame]).abs() <= 1.0e-6);
            let relative = config.region_offset + frame as u64;
            let loop_sample = relative % config.region_length;
            let seconds = loop_sample as f64 / config.sample_rate;
            let mut expected_indices = [-1i64; 3];
            unsafe {
                audio_note_segment_lookup_reference(
                    note_ranges.as_ptr(),
                    note_ranges.len(),
                    seconds,
                    expected_indices.as_mut_ptr(),
                );
            }
            assert_eq!(output_matched[frame], expected_indices[0]);
            assert_eq!(output_previous[frame], expected_indices[1]);
            if expected_indices[0] >= 0 {
                let curve = &curves[expected_indices[0] as usize];
                let mut expected_curve = [0.0f64; 3];
                unsafe {
                    audio_note_curve_reference_evaluate(
                        curve.anchors,
                        curve.anchor_count,
                        curve.integral_prefix,
                        curve.start_seconds,
                        curve.end_seconds,
                        curve.pitch_offset_cents,
                        curve.formant_offset_cents,
                        seconds,
                        expected_curve.as_mut_ptr(),
                    );
                }
                assert!((output_pitch[frame] - expected_curve[0]).abs() <= 1.0e-10);
                assert!((output_formant[frame] - expected_curve[1]).abs() <= 1.0e-10);
            } else {
                assert_eq!((output_pitch[frame], output_formant[frame]), (0.0, 0.0));
            }

            let expected_position = unsafe {
                hirari_region_source_position_reference(
                    warp_markers.as_ptr(),
                    warp_markers.len(),
                    loop_sample,
                    config.source_rate,
                    config.source_span,
                )
            };
            let expected_rate = if loop_sample + 1 < config.region_length {
                unsafe {
                    hirari_region_source_position_reference(
                        warp_markers.as_ptr(),
                        warp_markers.len(),
                        loop_sample + 1,
                        config.source_rate,
                        config.source_span,
                    ) - expected_position
                }
            } else if loop_sample > 0 {
                expected_position
                    - unsafe {
                        hirari_region_source_position_reference(
                            warp_markers.as_ptr(),
                            warp_markers.len(),
                            loop_sample - 1,
                            config.source_rate,
                            config.source_span,
                        )
                    }
            } else {
                1.0
            };
            assert!((output_positions[frame] - expected_position).abs() <= 1.0e-12);
            assert!((output_rates[frame] - expected_rate).abs() <= 1.0e-12);
        }
    }

    #[test]
    fn applies_gain_and_edge_fades() {
        let processor = RegionProcessorOrchestrator {
            gain: 2.0,
            fade_in: FadeInfo {
                duration_samples: 2,
                fade_type: FadeType::Linear,
            },
            fade_out: FadeInfo {
                duration_samples: 2,
                fade_type: FadeType::Linear,
            },
            length_samples: 4,
        };
        let mut buffer = [1.0; 4];
        processor.process_signal(&mut buffer, 0, 4, 0);
        assert_eq!(buffer, [0.0, 1.0, 2.0, 1.0]);
    }

    #[test]
    fn clamps_invalid_range_without_panicking() {
        let processor = RegionProcessorOrchestrator::new();
        let mut buffer = [f32::NAN, 1.0, 2.0];
        processor.process_signal(&mut buffer, 2, 100, 0);
        assert!(buffer[0].is_nan());
        assert_eq!(&buffer[1..], &[1.0, 2.0]);
    }

    #[test]
    fn processing_records_event_history_after_validation() {
        let mut processor = RegionProcessorOrchestrator::new();
        assert!(processor.set_gain_db(6.0));
        let mut history = RegionEditHistory::new();
        let mut buffer = [1.0; 2];
        let id = processor
            .process_with_history(&mut buffer, 0, 2, 0, "clip gain", &mut history)
            .unwrap();
        assert_eq!(id, 1);
        assert_eq!(
            history.entries().last().map(|entry| entry.label.as_str()),
            Some("clip gain")
        );
        assert!(buffer[0] > 1.0);
    }
