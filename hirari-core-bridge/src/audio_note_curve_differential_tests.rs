use super::{
    hirari_audio_note_curve_at, hirari_audio_note_curve_block,
    hirari_audio_note_curve_build_integral_prefix, hirari_audio_note_curve_integral_at,
    hirari_audio_note_find_segment_ranges, HirariAudioNoteAnchor, HirariAudioNoteCurveView,
    HirariAudioNoteSegmentRange,
};

unsafe extern "C" {
    fn audio_note_curve_reference_build_prefix(
        anchors: *const HirariAudioNoteAnchor,
        count: usize,
        pitch_offset: f64,
        output: *mut f64,
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
    fn audio_note_curve_cpp_wrapper_probe(output: *mut f64);
    fn audio_note_segment_lookup_reference(
        ranges: *const HirariAudioNoteSegmentRange,
        count: usize,
        seconds: f64,
        output: *mut i64,
    );
}

fn compare_curve(anchors: &[HirariAudioNoteAnchor], pitch_offset: f64, formant_offset: f64) {
    const START: f64 = 0.0;
    const FINISH: f64 = 1.0;
    let anchor_pointer = if anchors.is_empty() {
        std::ptr::null()
    } else {
        anchors.as_ptr()
    };
    let mut rust_prefix = vec![0.0; anchors.len()];
    let mut cpp_prefix = vec![0.0; anchors.len()];
    unsafe {
        hirari_audio_note_curve_build_integral_prefix(
            anchor_pointer,
            anchors.len(),
            pitch_offset,
            rust_prefix.as_mut_ptr(),
        );
        audio_note_curve_reference_build_prefix(
            anchor_pointer,
            anchors.len(),
            pitch_offset,
            cpp_prefix.as_mut_ptr(),
        );
    }
    for (index, (rust, cpp)) in rust_prefix.iter().zip(&cpp_prefix).enumerate() {
        assert!(
            (rust - cpp).abs() <= 1.0e-12,
            "prefix[{index}]: Rust={rust}, C++={cpp}"
        );
    }

    for seconds in [-0.25, 0.0, 0.05, 0.1, 0.2, 0.3, 0.45, 0.7, 0.9, 1.0, 1.2] {
        let mut rust_values = [0.0; 2];
        let mut cpp_values = [0.0; 3];
        let rust_integral = unsafe {
            hirari_audio_note_curve_integral_at(
                anchor_pointer,
                anchors.len(),
                rust_prefix.as_ptr(),
                START,
                FINISH,
                pitch_offset,
                seconds,
            )
        };
        unsafe {
            hirari_audio_note_curve_at(
                anchor_pointer,
                anchors.len(),
                seconds,
                pitch_offset,
                formant_offset,
                rust_values.as_mut_ptr(),
            );
            audio_note_curve_reference_evaluate(
                anchor_pointer,
                anchors.len(),
                cpp_prefix.as_ptr(),
                START,
                FINISH,
                pitch_offset,
                formant_offset,
                seconds,
                cpp_values.as_mut_ptr(),
            );
        }
        for (index, (rust, cpp)) in rust_values.iter().zip(&cpp_values[..2]).enumerate() {
            assert!(
                (rust - cpp).abs() <= 1.0e-10,
                "curve[{index}] at {seconds}: Rust={rust}, C++={cpp}"
            );
        }
        let rust_integral_with_cpp_prefix = unsafe {
            hirari_audio_note_curve_integral_at(
                anchor_pointer,
                anchors.len(),
                cpp_prefix.as_ptr(),
                START,
                FINISH,
                pitch_offset,
                seconds,
            )
        };
        assert!(
            (rust_integral - cpp_values[2]).abs() <= 1.0e-10
                && (rust_integral_with_cpp_prefix - cpp_values[2]).abs() <= 1.0e-10,
            "integral at {seconds}: Rust={rust_integral}, Rust+CPP-prefix={rust_integral_with_cpp_prefix}, C++={}",
            cpp_values[2]
        );
    }
}

#[test]
fn rust_audio_note_curves_match_frozen_cpp_interpolation_and_integral() {
    assert_eq!(std::mem::size_of::<HirariAudioNoteAnchor>(), 24);
    compare_curve(&[], 0.0, 0.0);
    compare_curve(
        &[HirariAudioNoteAnchor {
            position_seconds: 0.4,
            pitch_cents: 350.0,
            formant_cents: -125.0,
        }],
        -75.0,
        240.0,
    );
    compare_curve(
        &[
            HirariAudioNoteAnchor {
                position_seconds: 0.1,
                pitch_cents: -2_400.0,
                formant_cents: 300.0,
            },
            HirariAudioNoteAnchor {
                position_seconds: 0.3,
                pitch_cents: -25.0,
                formant_cents: -600.0,
            },
            HirariAudioNoteAnchor {
                position_seconds: 0.7,
                pitch_cents: 1_700.0,
                formant_cents: 1_200.0,
            },
            HirariAudioNoteAnchor {
                position_seconds: 0.95,
                pitch_cents: 4_900.0,
                formant_cents: -2_500.0,
            },
        ],
        240.0,
        -180.0,
    );
    compare_curve(
        &[
            HirariAudioNoteAnchor {
                position_seconds: 0.0,
                pitch_cents: 4_800.0,
                formant_cents: 2_400.0,
            },
            HirariAudioNoteAnchor {
                position_seconds: 1.0,
                pitch_cents: -4_800.0,
                formant_cents: -2_400.0,
            },
        ],
        4_800.0,
        -2_400.0,
    );
}

#[test]
fn cpp_audio_note_segment_wrapper_routes_to_rust_curve_kernels() {
    let anchors = [
        HirariAudioNoteAnchor {
            position_seconds: 0.2,
            pitch_cents: -300.0,
            formant_cents: 100.0,
        },
        HirariAudioNoteAnchor {
            position_seconds: 0.8,
            pitch_cents: 600.0,
            formant_cents: -250.0,
        },
    ];
    let mut prefix = [0.0; 2];
    let mut pitch_formant = [0.0; 2];
    unsafe {
        hirari_audio_note_curve_build_integral_prefix(
            anchors.as_ptr(),
            anchors.len(),
            100.0,
            prefix.as_mut_ptr(),
        );
        hirari_audio_note_curve_at(
            anchors.as_ptr(),
            anchors.len(),
            0.45,
            100.0,
            -50.0,
            pitch_formant.as_mut_ptr(),
        );
    }
    let integral = unsafe {
        hirari_audio_note_curve_integral_at(
            anchors.as_ptr(),
            anchors.len(),
            prefix.as_ptr(),
            0.0,
            1.0,
            100.0,
            0.45,
        )
    };
    let mut cpp_wrapper = [0.0; 3];
    unsafe { audio_note_curve_cpp_wrapper_probe(cpp_wrapper.as_mut_ptr()) };
    for (rust, cpp) in [pitch_formant[0], pitch_formant[1], integral]
        .iter()
        .zip(cpp_wrapper)
    {
        assert!(
            (rust - cpp).abs() <= 1.0e-10,
            "Rust={rust}, C++ wrapper={cpp}"
        );
    }
}

#[test]
fn rust_audio_note_segment_lookup_matches_cpp_overlap_and_gap_rules() {
    assert_eq!(std::mem::size_of::<HirariAudioNoteSegmentRange>(), 24);
    let fixture_sets = [
        vec![],
        vec![
            HirariAudioNoteSegmentRange {
                start_seconds: 0.0,
                end_seconds: 1.0,

                detected_pitch_cents: 0.0,
            },
            HirariAudioNoteSegmentRange {
                start_seconds: 1.0,
                end_seconds: 2.0,

                detected_pitch_cents: 0.0,
            },
            HirariAudioNoteSegmentRange {
                start_seconds: 3.0,
                end_seconds: 4.0,

                detected_pitch_cents: 0.0,
            },
        ],
        vec![
            HirariAudioNoteSegmentRange {
                start_seconds: 0.0,
                end_seconds: 2.0,

                detected_pitch_cents: 0.0,
            },
            HirariAudioNoteSegmentRange {
                start_seconds: 1.0,
                end_seconds: 1.5,

                detected_pitch_cents: 0.0,
            },
            HirariAudioNoteSegmentRange {
                start_seconds: 1.25,
                end_seconds: 3.0,

                detected_pitch_cents: 0.0,
            },
        ],
        vec![
            HirariAudioNoteSegmentRange {
                start_seconds: 0.0,
                end_seconds: 3.0,

                detected_pitch_cents: 0.0,
            },
            HirariAudioNoteSegmentRange {
                start_seconds: 1.0,
                end_seconds: 1.5,

                detected_pitch_cents: 0.0,
            },
        ],
    ];
    let queries = [f64::NAN, -0.1, 0.0, 0.5, 1.0, 1.4, 1.75, 2.0, 2.5, 3.0, 4.0];
    for ranges in fixture_sets {
        let pointer = if ranges.is_empty() {
            std::ptr::null()
        } else {
            ranges.as_ptr()
        };
        for seconds in queries {
            let mut rust = [-1i64; 3];
            let mut cpp = [-1i64; 3];
            unsafe {
                hirari_audio_note_find_segment_ranges(
                    pointer,
                    ranges.len(),
                    seconds,
                    rust.as_mut_ptr(),
                );
                audio_note_segment_lookup_reference(
                    pointer,
                    ranges.len(),
                    seconds,
                    cpp.as_mut_ptr(),
                );
            }
            assert_eq!(rust, cpp, "ranges={ranges:?}, seconds={seconds}");
        }
    }
}

#[test]
fn rust_track_curve_block_matches_frozen_cpp_sample_lookup_and_evaluation() {
    let ranges = [
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
        HirariAudioNoteSegmentRange {
            start_seconds: 0.009,
            end_seconds: 0.012,

            detected_pitch_cents: 0.0,
        },
    ];
    let anchors = [
        vec![
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
        vec![
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
        vec![HirariAudioNoteAnchor {
            position_seconds: 0.011,
            pitch_cents: 900.0,
            formant_cents: -600.0,
        }],
    ];
    let pitch_offsets = [85.0, -40.0, 120.0];
    let formant_offsets = [-30.0, 65.0, 240.0];
    let mut prefixes = [vec![0.0; 2], vec![0.0; 2], vec![0.0; 1]];
    let mut curves = Vec::with_capacity(anchors.len());
    for index in 0..anchors.len() {
        unsafe {
            hirari_audio_note_curve_build_integral_prefix(
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
            start_seconds: ranges[index].start_seconds,
            end_seconds: ranges[index].end_seconds,
            pitch_offset_cents: pitch_offsets[index],
            correction_before_seconds: 0.0,
            formant_offset_cents: formant_offsets[index],
        });
    }

    const FIRST: u64 = 2;
    const REGION_LENGTH: u64 = 12;
    const FRAMES: usize = 16;
    let mut pitch = [0.0; FRAMES];
    let mut formant = [0.0; FRAMES];
    let mut matched = [-1; FRAMES];
    let mut previous = [-1; FRAMES];
    unsafe {
        hirari_audio_note_curve_block(
            ranges.as_ptr(),
            ranges.len(),
            curves.as_ptr(),
            curves.len(),
            FIRST,
            REGION_LENGTH,
            1_000.0,
            FRAMES as u32,
            pitch.as_mut_ptr(),
            formant.as_mut_ptr(),
            matched.as_mut_ptr(),
            previous.as_mut_ptr(),
        );
    }
    for frame in 0..FRAMES {
        let seconds = ((FIRST + frame as u64) % REGION_LENGTH) as f64 / 1_000.0;
        let mut reference_indices = [-1; 3];
        unsafe {
            audio_note_segment_lookup_reference(
                ranges.as_ptr(),
                ranges.len(),
                seconds,
                reference_indices.as_mut_ptr(),
            );
        }
        assert_eq!(matched[frame], reference_indices[0], "frame={frame}");
        assert_eq!(previous[frame], reference_indices[1], "frame={frame}");
        if reference_indices[0] < 0 {
            assert_eq!((pitch[frame], formant[frame]), (0.0, 0.0));
            continue;
        }
        let curve_index = reference_indices[0] as usize;
        let mut expected = [0.0; 3];
        unsafe {
            audio_note_curve_reference_evaluate(
                curves[curve_index].anchors,
                curves[curve_index].anchor_count,
                curves[curve_index].integral_prefix,
                curves[curve_index].start_seconds,
                curves[curve_index].end_seconds,
                curves[curve_index].pitch_offset_cents,
                curves[curve_index].formant_offset_cents,
                seconds,
                expected.as_mut_ptr(),
            );
        }
        assert!((pitch[frame] - expected[0]).abs() <= 1.0e-10);
        assert!((formant[frame] - expected[1]).abs() <= 1.0e-10);
    }
}
