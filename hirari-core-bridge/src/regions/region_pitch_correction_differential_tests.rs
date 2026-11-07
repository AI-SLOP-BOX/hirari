use super::{
    hirari_region_pitch_corrected_frame, hirari_region_pitch_corrected_frame_with_curves,
    hirari_region_pitch_correction_delays,
};
use crate::audio_note_curve::{
    hirari_audio_note_curve_build_integral_prefix, HirariAudioNoteAnchor, HirariAudioNoteCurveView,
};

unsafe extern "C" {
    fn hirari_region_resampler_reference_kernel(output: *mut f32, capacity: usize) -> bool;
    fn region_pitch_correction_reference_delays(
        sample_rate: f64,
        reference_pitch_cents: f64,
        output: *mut f64,
    ) -> bool;
    fn region_pitch_correction_reference_frame(
        source_left: *const f32,
        source_right: *const f32,
        source_samples: u64,
        source_offset: u64,
        source_span: u64,
        warped_position: f64,
        local_source_rate: f64,
        effective_pitch_ratio: f64,
        base_pitch_ratio: f64,
        loop_relative: u64,
        note_correction_seconds: f64,
        sample_rate: f64,
        minimum_delay: f64,
        delay_range: f64,
        reverse: u8,
        kernel: *const f32,
        output: *mut f32,
    ) -> bool;
    fn audio_note_phase_correction_reference(
        seconds: f64,
        matched: *const HirariAudioNoteCurveView,
        previous: *const HirariAudioNoteCurveView,
    ) -> f64;
}

fn fixture(kind: usize, length: usize) -> (Vec<f32>, Vec<f32>) {
    let mut left = vec![0.0f32; length];
    let mut right = vec![0.0f32; length];
    for index in 0..length {
        let phase = index as f32;
        let noise = (((index as u32)
            .wrapping_mul(747_796_405)
            .wrapping_add(2_891_336_453)
            >> 16) as i16) as f32
            / i16::MAX as f32;
        match kind {
            0 if index == 1_004 || index == 1_760 => {
                left[index] = if index == 1_004 { 1.0 } else { -0.75 };
                right[index] = if index == 1_004 { -0.5 } else { 0.9 };
            }
            1 => {
                left[index] = 0.7 * (phase * 0.071).sin();
                right[index] = 0.4 * (phase * 0.043).sin();
            }
            2 => {
                left[index] = noise * 0.5;
                right[index] = noise * -0.25;
            }
            _ => {}
        }
    }
    if kind == 2 {
        left[1_100] = f32::NAN;
        right[1_400] = f32::INFINITY;
    }
    (left, right)
}

#[test]
fn rust_pitch_correction_delay_window_matches_frozen_cpp() {
    for sample_rate in [44_100.0, 48_000.0, 96_000.0] {
        for pitch_cents in [0.0, 5700.0, 6900.0, 8400.0, f64::NAN] {
            let mut rust = [0.0; 3];
            let mut cpp = [0.0; 3];
            unsafe {
                assert!(hirari_region_pitch_correction_delays(
                    sample_rate,
                    pitch_cents,
                    rust.as_mut_ptr(),
                ));
                assert!(region_pitch_correction_reference_delays(
                    sample_rate,
                    pitch_cents,
                    cpp.as_mut_ptr(),
                ));
            }
            assert!((rust[0] - cpp[0]).abs() <= 1.0e-10);
            assert!((rust[1] - cpp[1]).abs() <= 1.0e-10);
            assert!((rust[2] - cpp[2]).abs() <= 1.0e-10);
        }
    }
}

#[test]
fn rust_pitch_corrected_stereo_frame_matches_frozen_cpp_two_tap_renderer() {
    const SOURCE_OFFSET: u64 = 73;
    const SOURCE_SPAN: u64 = 3_500;
    let mut cpp_kernel = vec![0.0f32; 17 * 64 * 8];
    let rust_kernel = super::super::region_resampler::hirari_region_resampler_prepare();
    unsafe {
        assert!(hirari_region_resampler_reference_kernel(
            cpp_kernel.as_mut_ptr(),
            cpp_kernel.len(),
        ));
    }

    for fixture_id in 0..4 {
        let (left, right) = fixture(fixture_id, 4_096);
        for reverse in [0u8, 1] {
            for local_rate in [0.5, 1.0, 1.25] {
                for base_ratio in [0.5, 1.0, 1.5] {
                    for pitch_ratio in [0.5, 1.25, 2.0] {
                        for loop_relative in [0u64, 1_024, 48_000] {
                            for correction_seconds in [-0.1, 0.0, 0.25] {
                                let mut delay = [0.0f64; 2];
                                assert!(unsafe {
                                    hirari_region_pitch_correction_delays(
                                        48_000.0,
                                        6_900.0,
                                        delay.as_mut_ptr(),
                                    )
                                });
                                let args = (
                                    left.as_ptr(),
                                    right.as_ptr(),
                                    left.len() as u64,
                                    SOURCE_OFFSET,
                                    SOURCE_SPAN,
                                    1_500.25,
                                    local_rate,
                                    pitch_ratio,
                                    base_ratio,
                                    loop_relative,
                                    correction_seconds,
                                    48_000.0,
                                    delay[0],
                                    delay[1],
                                    reverse,
                                );
                                let mut rust = [0.0f32; 4];
                                let mut cpp = [0.0f32; 4];
                                unsafe {
                                    assert!(hirari_region_pitch_corrected_frame(
                                        args.0,
                                        args.1,
                                        args.2,
                                        args.3,
                                        args.4,
                                        args.5,
                                        args.6,
                                        args.7,
                                        args.8,
                                        args.9,
                                        args.10,
                                        args.11,
                                        args.12,
                                        args.13,
                                        args.14,
                                        rust_kernel,
                                        rust.as_mut_ptr(),
                                    ));
                                    assert!(region_pitch_correction_reference_frame(
                                        args.0,
                                        args.1,
                                        args.2,
                                        args.3,
                                        args.4,
                                        args.5,
                                        args.6,
                                        args.7,
                                        args.8,
                                        args.9,
                                        args.10,
                                        args.11,
                                        args.12,
                                        args.13,
                                        args.14,
                                        cpp_kernel.as_ptr(),
                                        cpp.as_mut_ptr(),
                                    ));
                                }
                                for channel in 0..4 {
                                    assert!(
                                        (rust[channel] - cpp[channel]).abs() <= 5.0e-6,
                                        "fixture={fixture_id}, reverse={reverse}, local_rate={local_rate}, base={base_ratio}, pitch={pitch_ratio}, loop={loop_relative}, correction={correction_seconds}, output={channel}: Rust={}, C++={}",
                                        rust[channel], cpp[channel]
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn rust_curve_phase_and_pitch_corrected_frame_match_frozen_cpp() {
    let (left, right) = fixture(1, 4_096);
    let rust_kernel = super::super::region_resampler::hirari_region_resampler_prepare();
    let mut cpp_kernel = vec![0.0f32; 17 * 64 * 8];
    unsafe {
        assert!(hirari_region_resampler_reference_kernel(
            cpp_kernel.as_mut_ptr(),
            cpp_kernel.len(),
        ));
    }

    let matched_anchors = [
        HirariAudioNoteAnchor {
            position_seconds: 0.2,
            pitch_cents: -350.0,
            formant_cents: 0.0,
        },
        HirariAudioNoteAnchor {
            position_seconds: 0.45,
            pitch_cents: 720.0,
            formant_cents: 0.0,
        },
        HirariAudioNoteAnchor {
            position_seconds: 0.8,
            pitch_cents: 125.0,
            formant_cents: 0.0,
        },
    ];
    let previous_anchors = [
        HirariAudioNoteAnchor {
            position_seconds: 0.0,
            pitch_cents: 240.0,
            formant_cents: 0.0,
        },
        HirariAudioNoteAnchor {
            position_seconds: 0.21,
            pitch_cents: -180.0,
            formant_cents: 0.0,
        },
        HirariAudioNoteAnchor {
            position_seconds: 0.4,
            pitch_cents: 510.0,
            formant_cents: 0.0,
        },
    ];
    let mut matched_prefix = [0.0; 3];
    let mut previous_prefix = [0.0; 3];
    unsafe {
        hirari_audio_note_curve_build_integral_prefix(
            matched_anchors.as_ptr(),
            matched_anchors.len(),
            85.0,
            matched_prefix.as_mut_ptr(),
        );
        hirari_audio_note_curve_build_integral_prefix(
            previous_anchors.as_ptr(),
            previous_anchors.len(),
            -65.0,
            previous_prefix.as_mut_ptr(),
        );
    }
    let matched = HirariAudioNoteCurveView {
        anchors: matched_anchors.as_ptr(),
        anchor_count: matched_anchors.len(),
        integral_prefix: matched_prefix.as_ptr(),
        start_seconds: 0.2,
        end_seconds: 0.8,
        pitch_offset_cents: 85.0,
        correction_before_seconds: -0.037,
        formant_offset_cents: 32.0,
    };
    let previous = HirariAudioNoteCurveView {
        anchors: previous_anchors.as_ptr(),
        anchor_count: previous_anchors.len(),
        integral_prefix: previous_prefix.as_ptr(),
        start_seconds: 0.0,
        end_seconds: 0.4,
        pitch_offset_cents: -65.0,
        correction_before_seconds: 0.112,
        formant_offset_cents: -18.0,
    };
    assert_eq!(std::mem::size_of::<HirariAudioNoteCurveView>(), 64);

    let cases = [
        (None, None),
        (Some(&matched), None),
        (None, Some(&previous)),
        (Some(&matched), Some(&previous)),
    ];
    for (matched_view, previous_view) in cases {
        for note_seconds in [0.0, 0.2, 0.45, 0.8, 1.0] {
            let matched_ptr = matched_view.map_or(std::ptr::null(), |view| view as *const _);
            let previous_ptr = previous_view.map_or(std::ptr::null(), |view| view as *const _);
            let correction = unsafe {
                audio_note_phase_correction_reference(note_seconds, matched_ptr, previous_ptr)
            };
            let mut delays = [0.0; 2];
            assert!(unsafe {
                hirari_region_pitch_correction_delays(48_000.0, 6_900.0, delays.as_mut_ptr())
            });
            let args = (
                left.as_ptr(),
                right.as_ptr(),
                left.len() as u64,
                73,
                3_500,
                1_500.25,
                1.125,
                1.25,
                1.0,
                1_024,
                note_seconds,
                48_000.0,
                delays[0],
                delays[1],
                0,
            );
            let mut rust = [0.0f32; 4];
            let mut cpp = [0.0f32; 4];
            unsafe {
                assert!(hirari_region_pitch_corrected_frame_with_curves(
                    args.0,
                    args.1,
                    args.2,
                    args.3,
                    args.4,
                    args.5,
                    args.6,
                    args.7,
                    args.8,
                    args.9,
                    args.10,
                    args.11,
                    args.12,
                    args.13,
                    args.14,
                    rust_kernel,
                    matched_ptr,
                    previous_ptr,
                    rust.as_mut_ptr(),
                ));
                assert!(region_pitch_correction_reference_frame(
                    args.0,
                    args.1,
                    args.2,
                    args.3,
                    args.4,
                    args.5,
                    args.6,
                    args.7,
                    args.8,
                    args.9,
                    correction,
                    args.11,
                    args.12,
                    args.13,
                    args.14,
                    cpp_kernel.as_ptr(),
                    cpp.as_mut_ptr(),
                ));
            }
            for channel in 0..4 {
                assert!(
                    (rust[channel] - cpp[channel]).abs() <= 5.0e-6,
                    "matched={}, previous={}, seconds={note_seconds}, channel={channel}: Rust={}, C++={}",
                    matched_view.is_some(), previous_view.is_some(), rust[channel], cpp[channel]
                );
            }
        }
    }
}
