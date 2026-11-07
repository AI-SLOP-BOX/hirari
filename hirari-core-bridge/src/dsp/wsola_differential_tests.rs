use super::{hirari_region_wsola_frame, hirari_wsola_select_grain};

unsafe extern "C" {
    fn hirari_region_resampler_reference_kernel(output: *mut f32, capacity: usize) -> bool;
    fn hirari_wsola_select_grain_reference(
        left: *const f32,
        right: *const f32,
        source_samples: u64,
        source_offset: u64,
        source_span: u64,
        expected: f64,
        reference: f64,
        overlap_span: f64,
        reference_span: f64,
        center: i64,
        last: i64,
        search_radius: i64,
        reverse: u8,
        current_offsets: *const f64,
        previous_offsets: *const f64,
        point_count: u32,
    ) -> f64;
    fn hirari_wsola_render_frame_reference(
        left: *const f32,
        right: *const f32,
        source_samples: u64,
        source_offset: u64,
        source_span: u64,
        reverse: u8,
        resample_step: f64,
        kernel: *const f32,
        positions: *const f64,
        weights: *const f32,
        grain_count: u32,
        output: *mut f32,
    );
    fn hirari_region_wsola_frame_reference(
        left: *const f32,
        right: *const f32,
        source_samples: u64,
        source_offset: u64,
        source_span: u64,
        region_length: u64,
        loop_relative: u64,
        sample_rate: f64,
        source_rate: f64,
        base_pitch_ratio: f64,
        effective_pitch_ratio: f64,
        cache_grain_ids: *mut u64,
        cache_grain_starts: *mut f64,
        cache_capacity: usize,
        sync_group: u32,
        reverse: u8,
        kernel: *const f32,
        window: *const f32,
        output: *mut f32,
    );
}

fn fixture(kind: usize, length: usize) -> (Vec<f32>, Vec<f32>) {
    let mut left = vec![0.0; length];
    let mut right = vec![0.0; length];
    let mut pink = 0.0f32;
    for index in 0..length {
        let time = index as f32;
        let noise = (((index as u32)
            .wrapping_mul(747_796_405)
            .wrapping_add(2_891_336_453)
            >> 16) as i16) as f32
            / i16::MAX as f32;
        pink = 0.97 * pink + 0.03 * noise;
        match kind {
            0 => {
                if index == 997 || index == 1_821 {
                    left[index] = if index == 997 { 1.0 } else { -0.8 };
                    right[index] = if index == 997 { -0.5 } else { 0.9 };
                }
            }
            1 => {
                left[index] = 0.7 * (time * 0.071).sin();
                right[index] = 0.4 * (time * 0.043).sin();
            }
            2 => {
                left[index] = noise * 0.5;
                right[index] = pink * 0.4;
            }
            3 => {}
            _ => {
                left[index] = 0.3 * (time * 0.03).sin() + 0.1 * (time * 0.07).cos();
                right[index] = -0.2 * (time * 0.041).sin();
            }
        }
    }
    if kind == 4 {
        left[1100] = f32::NAN;
        right[1400] = f32::INFINITY;
    }
    (left, right)
}

#[test]
fn rust_wsola_grain_selection_matches_frozen_cpp_correlation_search() {
    const SOURCE_OFFSET: u64 = 73;
    const SOURCE_SPAN: u64 = 3_500;
    const EXPECTED: f64 = 1_000.0;
    const REFERENCE: f64 = 450.0;
    const POINTS: usize = 32;

    for fixture_id in 0..5 {
        let (left, right) = fixture(fixture_id, 4_096);
        for reverse in [0u8, 1] {
            for rate in [0.5f64, 0.9, 1.0, 1.2, 1.5] {
                for search_radius in [32i64, 64, 96] {
                    let current_offsets = (0..POINTS)
                        .map(|index| index as f64 * 8.0 * rate)
                        .collect::<Vec<_>>();
                    let previous_offsets = (0..POINTS)
                        .map(|index| index as f64 * 8.0 * rate)
                        .collect::<Vec<_>>();
                    let overlap_span = 248.0 * rate;
                    let reference_span = 248.0 * rate;
                    let args = (
                        left.as_ptr(),
                        right.as_ptr(),
                        left.len() as u64,
                        SOURCE_OFFSET,
                        SOURCE_SPAN,
                        EXPECTED,
                        REFERENCE,
                        overlap_span,
                        reference_span,
                        EXPECTED.round() as i64,
                        (SOURCE_SPAN - 1) as i64,
                        search_radius,
                        reverse,
                        current_offsets.as_ptr(),
                        previous_offsets.as_ptr(),
                        POINTS as u32,
                    );
                    let rust = unsafe {
                        hirari_wsola_select_grain(
                            args.0, args.1, args.2, args.3, args.4, args.5, args.6, args.7, args.8,
                            args.9, args.10, args.11, args.12, args.13, args.14, args.15,
                        )
                    };
                    let cpp = unsafe {
                        hirari_wsola_select_grain_reference(
                            args.0, args.1, args.2, args.3, args.4, args.5, args.6, args.7, args.8,
                            args.9, args.10, args.11, args.12, args.13, args.14, args.15,
                        )
                    };
                    assert_eq!(rust, cpp,
                        "fixture={fixture_id}, reverse={reverse}, rate={rate}, radius={search_radius}");
                }
            }
        }
    }
}

#[test]
fn rust_wsola_invalid_input_falls_back_to_nominal_grain_start() {
    let expected = 123.5;
    let selected = unsafe {
        hirari_wsola_select_grain(
            std::ptr::null(),
            std::ptr::null(),
            0,
            0,
            0,
            expected,
            0.0,
            0.0,
            0.0,
            123,
            0,
            0,
            0,
            std::ptr::null(),
            std::ptr::null(),
            0,
        )
    };
    assert_eq!(selected, expected);
}

#[test]
fn rust_wsola_overlap_add_matches_frozen_cpp_for_stereo_frames() {
    let rust_kernel = super::super::region_resampler::hirari_region_resampler_prepare();
    let mut cpp_kernel = vec![0.0f32; 17 * 64 * 8];
    unsafe {
        assert!(hirari_region_resampler_reference_kernel(
            cpp_kernel.as_mut_ptr(),
            cpp_kernel.len()
        ));
    }

    for fixture_id in 0..5 {
        let (left, right) = fixture(fixture_id, 4_096);
        for reverse in [0u8, 1] {
            for step in [1.0, 1.25, 2.25] {
                for grain_count in 1..=4usize {
                    let positions = [300.25, 360.5, 420.75, 480.125];
                    let weights = [0.5, 0.75, 0.8, 0.25];
                    let mut rust = [0.0f32; 4];
                    let mut cpp = [0.0f32; 4];
                    unsafe {
                        super::hirari_wsola_render_frame(
                            left.as_ptr(),
                            right.as_ptr(),
                            left.len() as u64,
                            73,
                            3_500,
                            reverse,
                            step,
                            rust_kernel,
                            positions.as_ptr(),
                            weights.as_ptr(),
                            grain_count as u32,
                            rust.as_mut_ptr(),
                        );
                        hirari_wsola_render_frame_reference(
                            left.as_ptr(),
                            right.as_ptr(),
                            left.len() as u64,
                            73,
                            3_500,
                            reverse,
                            step,
                            cpp_kernel.as_ptr(),
                            positions.as_ptr(),
                            weights.as_ptr(),
                            grain_count as u32,
                            cpp.as_mut_ptr(),
                        );
                    }
                    for channel in 0..4 {
                        assert!(
                            (rust[channel] - cpp[channel]).abs() <= 4.0e-6,
                            "fixture={fixture_id}, reverse={reverse}, step={step}, grains={grain_count}, output={channel}: Rust={}, C++={}",
                            rust[channel], cpp[channel]
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn rust_region_wsola_scheduler_matches_frozen_cpp_frame_path() {
    const REGION_LENGTH: u64 = 2_048;
    const SOURCE_OFFSET: u64 = 73;
    const SOURCE_SPAN: u64 = 4_096;
    const CACHE_CAPACITY: usize = 16;
    let (left, right) = fixture(2, 5_000);
    let rust_kernel = super::super::region_resampler::hirari_region_resampler_prepare();
    let mut cpp_kernel = vec![0.0f32; 17 * 64 * 8];
    unsafe {
        assert!(hirari_region_resampler_reference_kernel(
            cpp_kernel.as_mut_ptr(),
            cpp_kernel.len(),
        ));
    }
    let window = std::array::from_fn::<_, 1024, _>(|index| {
        0.5 * (1.0 - (6.28318530718f32 * index as f32 / 1023.0).cos())
    });

    for source_rate in [0.75, 1.0, 1.25] {
        for pitch_ratio in [0.75, 1.0, 1.4] {
            for sync_group in [0, 7] {
                for reverse in [0u8, 1] {
                    let mut rust_ids = [u64::MAX; CACHE_CAPACITY];
                    let mut cpp_ids = [u64::MAX; CACHE_CAPACITY];
                    let mut rust_starts = [0.0f64; CACHE_CAPACITY];
                    let mut cpp_starts = [0.0f64; CACHE_CAPACITY];
                    for loop_relative in [0u64, 255, 256, 511, 512, 1_023, 1_536] {
                        let mut rust = [0.0f32; 4];
                        let mut cpp = [0.0f32; 4];
                        unsafe {
                            hirari_region_wsola_frame(
                                left.as_ptr(),
                                right.as_ptr(),
                                left.len() as u64,
                                SOURCE_OFFSET,
                                SOURCE_SPAN,
                                REGION_LENGTH,
                                loop_relative,
                                48_000.0,
                                source_rate,
                                pitch_ratio,
                                pitch_ratio,
                                std::ptr::null(),
                                0,
                                std::ptr::null(),
                                std::ptr::null(),
                                0,
                                rust_ids.as_mut_ptr(),
                                rust_starts.as_mut_ptr(),
                                CACHE_CAPACITY,
                                sync_group,
                                reverse,
                                rust_kernel,
                                window.as_ptr(),
                                rust.as_mut_ptr(),
                            );
                            hirari_region_wsola_frame_reference(
                                left.as_ptr(),
                                right.as_ptr(),
                                left.len() as u64,
                                SOURCE_OFFSET,
                                SOURCE_SPAN,
                                REGION_LENGTH,
                                loop_relative,
                                48_000.0,
                                source_rate,
                                pitch_ratio,
                                pitch_ratio,
                                cpp_ids.as_mut_ptr(),
                                cpp_starts.as_mut_ptr(),
                                CACHE_CAPACITY,
                                sync_group,
                                reverse,
                                cpp_kernel.as_ptr(),
                                window.as_ptr(),
                                cpp.as_mut_ptr(),
                            );
                        }
                        for channel in 0..4 {
                            assert!(
                                (rust[channel] - cpp[channel]).abs() <= 4.0e-6,
                                "rate={source_rate}, pitch={pitch_ratio}, sync={sync_group}, reverse={reverse}, timeline={loop_relative}, channel={channel}: Rust={}, C++={}",
                                rust[channel], cpp[channel]
                            );
                        }
                        assert_eq!(rust_ids, cpp_ids);
                        assert_eq!(rust_starts, cpp_starts);
                    }
                }
            }
        }
    }
}
