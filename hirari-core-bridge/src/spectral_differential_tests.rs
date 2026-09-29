use super::hirari_spectral_processor_apply_gain;

unsafe extern "C" {
    fn spectral_reference_apply_gain(
        channels: *const *mut f32,
        channel_count: u32,
        frames: u32,
        sample_rate: f64,
        t0: f32,
        f0: f32,
        t1: f32,
        f1: f32,
        gain: f32,
    ) -> bool;
}

fn fixture(kind: usize, length: usize) -> [Vec<f32>; 2] {
    let mut channels = [vec![0.0; length], vec![0.0; length]];
    let mut pink = 0.0f32;
    for index in 0..length {
        let t = index as f32;
        let white = (((index as u32)
            .wrapping_mul(747_796_405)
            .wrapping_add(2_891_336_453)
            >> 16) as i16) as f32
            / i16::MAX as f32;
        pink = 0.97 * pink + 0.03 * white;
        match kind {
            0 if index == 0 => {
                channels[0][index] = 1.0;
                channels[1][index] = -0.5;
            }
            1 => {
                channels[0][index] = 0.7 * (t * 0.071).sin();
                channels[1][index] = 0.4 * (t * 0.053).sin();
            }
            2 => {
                channels[0][index] = white * 0.5;
                channels[1][index] = -white * 0.25;
            }
            3 => {
                channels[0][index] = pink;
                channels[1][index] = -0.6 * pink;
            }
            4 => {}
            5 => {
                channels[0][index] = (0.93 * (t * 0.13).sin()).clamp(-0.8, 0.8);
                channels[1][index] = channels[0][index];
            }
            6 if index == length / 2 => {
                channels[0][index] = 1.0;
                channels[1][index] = -1.0;
            }
            7 => {
                channels[0][index] =
                    0.4 * (t * 0.014).sin() + 0.2 * (t * 0.028).sin() + 0.1 * (t * 0.042).sin();
                channels[1][index] = channels[0][index];
            }
            8 => {
                channels[0][index] = white * 0.2;
                channels[1][index] = pink * 0.2;
            }
            _ => {}
        }
    }
    if kind == 8 && length > 64 {
        channels[0][17] = f32::NAN;
        channels[1][63] = f32::INFINITY;
    }
    channels
}

fn compare(reference: &[f32], rust: &[f32], kind: usize, length: usize, channel: usize) {
    let mut max_error = 0.0f32;
    let mut at = 0usize;
    for (index, (expected, actual)) in reference.iter().zip(rust).enumerate() {
        assert_eq!(expected.is_finite(), actual.is_finite(),
            "fixture={kind} frames={length} channel={channel} finite classification differs at {index}");
        if expected.is_finite() {
            let error = (expected - actual).abs();
            if error > max_error {
                max_error = error;
                at = index;
            }
        }
    }
    assert!(
        max_error <= 8.0e-5,
        "fixture={kind} frames={length} channel={channel} differs at {at} by {max_error}"
    );
}

#[test]
fn rust_spectral_gain_matches_frozen_cpp_stft_ola_reference() {
    const LENGTHS: [usize; 9] = [1, 63, 1023, 2047, 2048, 2049, 4095, 4096, 8193];
    for kind in 0..9 {
        for length in LENGTHS {
            let mut rust = fixture(kind, length);
            let mut reference = rust.clone();
            let mut rust_ptrs = [rust[0].as_mut_ptr(), rust[1].as_mut_ptr()];
            let mut reference_ptrs = [reference[0].as_mut_ptr(), reference[1].as_mut_ptr()];
            let args = (2, length as u32, 48_000.0, 0.0, 100.0, 0.15, 12_000.0, 0.23);
            let applied = unsafe {
                hirari_spectral_processor_apply_gain(
                    rust_ptrs.as_mut_ptr(),
                    args.0,
                    args.1,
                    args.2,
                    args.3,
                    args.4,
                    args.5,
                    args.6,
                    args.7,
                )
            };
            let reference_applied = unsafe {
                spectral_reference_apply_gain(
                    reference_ptrs.as_mut_ptr(),
                    args.0,
                    args.1,
                    args.2,
                    args.3,
                    args.4,
                    args.5,
                    args.6,
                    args.7,
                )
            };
            assert_eq!(applied, reference_applied, "fixture={kind} frames={length}");
            assert!(applied);
            for channel in 0..2 {
                compare(&reference[channel], &rust[channel], kind, length, channel);
            }
        }
    }
}

#[test]
fn rust_spectral_gain_matches_frozen_cpp_for_mono_stereo_and_four_channels() {
    const LENGTH: usize = 4097;
    for channel_count in [1usize, 2, 4] {
        let base = fixture(2, LENGTH);
        let mut rust = (0..channel_count)
            .map(|channel| {
                let mut samples = base[channel % 2].clone();
                let scale = 1.0 / (channel + 1) as f32;
                samples.iter_mut().for_each(|sample| *sample *= scale);
                samples
            })
            .collect::<Vec<_>>();
        let mut reference = rust.clone();
        let mut rust_ptrs = rust.iter_mut().map(Vec::as_mut_ptr).collect::<Vec<_>>();
        let mut reference_ptrs = reference
            .iter_mut()
            .map(Vec::as_mut_ptr)
            .collect::<Vec<_>>();
        let args = (
            channel_count as u32,
            LENGTH as u32,
            96_000.0,
            0.002,
            120.0,
            0.1,
            18_000.0,
            0.4,
        );
        let applied = unsafe {
            hirari_spectral_processor_apply_gain(
                rust_ptrs.as_mut_ptr(),
                args.0,
                args.1,
                args.2,
                args.3,
                args.4,
                args.5,
                args.6,
                args.7,
            )
        };
        let reference_applied = unsafe {
            spectral_reference_apply_gain(
                reference_ptrs.as_mut_ptr(),
                args.0,
                args.1,
                args.2,
                args.3,
                args.4,
                args.5,
                args.6,
                args.7,
            )
        };
        assert_eq!(applied, reference_applied, "channels={channel_count}");
        assert!(applied);
        for channel in 0..channel_count {
            compare(&reference[channel], &rust[channel], 2, LENGTH, channel);
        }
    }
}

#[test]
fn rust_fft_matches_frozen_cpp_reference_from_small_to_large_plans() {
    use crate::fft::{
        hirari_fft_forward, hirari_fft_inverse, hirari_fft_plan_create, hirari_fft_plan_destroy,
    };

    unsafe extern "C" {
        fn spectral_reference_fft(
            real: *mut f32,
            imag: *mut f32,
            size: usize,
            inverse: bool,
        ) -> bool;
    }

    for invalid in [0usize, 1, 3, 6, 1 << 21] {
        assert!(
            hirari_fft_plan_create(invalid).is_null(),
            "accepted invalid FFT size {invalid}"
        );
    }
    for size in [2usize, 16, 2048, 65_536, 262_144] {
        for signal in 0..3 {
            let mut initial_real = vec![0.0; size];
            let initial_imag = vec![0.0; size];
            for index in 0..size {
                let phase = index as f32;
                initial_real[index] = match signal {
                    0 => {
                        if index == 0 {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    1 => 0.6 * (phase * 0.013).sin() + 0.2 * (phase * 0.037).cos(),
                    _ => {
                        (((index as u32)
                            .wrapping_mul(747_796_405)
                            .wrapping_add(2_891_336_453)
                            >> 16) as i16) as f32
                            / i16::MAX as f32
                    }
                };
            }
            let mut rust_real = initial_real.clone();
            let mut rust_imag = initial_imag.clone();
            let mut cpp_real = initial_real;
            let mut cpp_imag = initial_imag;
            let plan = hirari_fft_plan_create(size);
            assert!(!plan.is_null(), "failed to create valid FFT size {size}");
            unsafe {
                hirari_fft_forward(plan, rust_real.as_mut_ptr(), rust_imag.as_mut_ptr());
                assert!(spectral_reference_fft(
                    cpp_real.as_mut_ptr(),
                    cpp_imag.as_mut_ptr(),
                    size,
                    false
                ));
            }
            for index in 0..size {
                let error = (rust_real[index] - cpp_real[index])
                    .abs()
                    .max((rust_imag[index] - cpp_imag[index]).abs());
                // Large transforms accumulate f32 butterfly rounding in the
                // raw bins. The time-domain round trip below has a tighter
                // absolute bound, and the STFT/OLA test checks rendered audio.
                let tolerance = 2.0e-3 + 2.0e-4 * cpp_real[index].abs().max(cpp_imag[index].abs());
                assert!(error <= tolerance,
                    "FFT forward mismatch size={size} signal={signal} bin={index} error={error} tolerance={tolerance}");
            }
            unsafe {
                hirari_fft_inverse(plan, rust_real.as_mut_ptr(), rust_imag.as_mut_ptr());
                assert!(spectral_reference_fft(
                    cpp_real.as_mut_ptr(),
                    cpp_imag.as_mut_ptr(),
                    size,
                    true
                ));
            }
            for index in 0..size {
                let error = (rust_real[index] - cpp_real[index])
                    .abs()
                    .max((rust_imag[index] - cpp_imag[index]).abs());
                assert!(
                    error <= 3.0e-5,
                    "FFT inverse mismatch size={size} signal={signal} sample={index} error={error}"
                );
            }
            unsafe { hirari_fft_plan_destroy(plan) };
        }
    }
}
