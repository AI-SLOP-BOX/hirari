/// Applies the legacy mid/side vocal-removal kernel in place.
#[no_mangle]
pub unsafe extern "C" fn hirari_vocal_remove_stereo(
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) -> bool {
    if left.is_null() || right.is_null() || frames == 0 {
        return false;
    }
    let left = std::slice::from_raw_parts_mut(left, frames);
    let right = std::slice::from_raw_parts_mut(right, frames);
    let mut bass_left = 0.0f32;
    let mut bass_right = 0.0f32;
    for frame in 0..frames {
        let input_left = left[frame];
        let input_right = right[frame];
        bass_left += 0.02 * (input_left - bass_left);
        bass_right += 0.02 * (input_right - bass_right);
        let vocal = input_left - input_right;
        left[frame] = vocal * 0.5 + bass_left * 0.5;
        right[frame] = -vocal * 0.5 + bass_right * 0.5;
    }
    true
}

/// Returns the normalized stereo cross-correlation used by the track analysis.
#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_correlation(
    left: *const f32,
    right: *const f32,
    frames: usize,
) -> f32 {
    if left.is_null() || right.is_null() || frames == 0 {
        return 1.0;
    }
    let left = std::slice::from_raw_parts(left, frames);
    let right = std::slice::from_raw_parts(right, frames);
    let mut sum_lr = 0.0f64;
    let mut sum_left = 0.0f64;
    let mut sum_right = 0.0f64;
    for frame in 0..frames {
        let l = left[frame] as f64;
        let r = right[frame] as f64;
        sum_lr += l * r;
        sum_left += l * l;
        sum_right += r * r;
    }
    let denominator = (sum_left * sum_right).sqrt();
    if denominator < 1.0e-9 {
        1.0
    } else {
        (sum_lr / denominator) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocal_removal_cancels_centered_high_frequency_and_preserves_low_component() {
        let mut left = Vec::with_capacity(1024);
        let mut right = Vec::with_capacity(1024);
        for index in 0..1024 {
            let vocal = if index % 2 == 0 { 0.8 } else { -0.8 };
            let bass = (index as f32 * 0.008).sin() * 0.2;
            left.push(vocal + bass);
            right.push(vocal - bass);
        }
        assert!(unsafe {
            hirari_vocal_remove_stereo(left.as_mut_ptr(), right.as_mut_ptr(), left.len())
        });
        assert!(left.iter().chain(&right).all(|sample| sample.is_finite()));
        assert!(left.iter().zip(&right).all(|(l, r)| (l - r).abs() < 1.0));
    }

    #[test]
    fn correlation_handles_silence_and_opposite_phase() {
        let silence = [0.0f32; 4];
        let signal = [1.0f32, -0.5, 0.25, -0.125];
        let opposite = signal.map(|sample| -sample);
        assert_eq!(
            unsafe { hirari_stereo_correlation(silence.as_ptr(), silence.as_ptr(), silence.len()) },
            1.0
        );
        assert!(
            (unsafe {
                hirari_stereo_correlation(signal.as_ptr(), opposite.as_ptr(), signal.len())
            } + 1.0)
                .abs()
                < 1.0e-6
        );
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn rust_stereo_kernels_match_frozen_cpp() {
        unsafe extern "C" {
            fn hirari_vocal_remove_stereo_reference(left: *mut f32, right: *mut f32, frames: usize);
            fn hirari_stereo_correlation_reference(
                left: *const f32,
                right: *const f32,
                frames: usize,
            ) -> f32;
        }

        let left_input = (0..1024)
            .map(|index| (index as f32 * 0.017).sin() * 0.7 + (index % 7) as f32 * 0.013)
            .collect::<Vec<_>>();
        let right_input = (0..1024)
            .map(|index| (index as f32 * 0.031).cos() * 0.5 - (index % 5) as f32 * 0.009)
            .collect::<Vec<_>>();
        let mut rust_left = left_input.clone();
        let mut rust_right = right_input.clone();
        let mut cpp_left = left_input.clone();
        let mut cpp_right = right_input.clone();
        assert!(unsafe {
            hirari_vocal_remove_stereo(rust_left.as_mut_ptr(), rust_right.as_mut_ptr(), 1024)
        });
        unsafe {
            hirari_vocal_remove_stereo_reference(
                cpp_left.as_mut_ptr(),
                cpp_right.as_mut_ptr(),
                1024,
            )
        };
        for (index, (rust, cpp)) in rust_left.iter().zip(&cpp_left).enumerate() {
            assert!(
                (rust - cpp).abs() <= 2.0e-7,
                "left sample={index}: {rust} vs {cpp}"
            );
        }
        for (index, (rust, cpp)) in rust_right.iter().zip(&cpp_right).enumerate() {
            assert!(
                (rust - cpp).abs() <= 2.0e-7,
                "right sample={index}: {rust} vs {cpp}"
            );
        }

        let rust_correlation =
            unsafe { hirari_stereo_correlation(left_input.as_ptr(), right_input.as_ptr(), 1024) };
        let cpp_correlation = unsafe {
            hirari_stereo_correlation_reference(left_input.as_ptr(), right_input.as_ptr(), 1024)
        };
        assert!((rust_correlation - cpp_correlation).abs() <= 1.0e-7);
    }
}
