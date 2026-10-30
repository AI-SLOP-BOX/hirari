#[no_mangle]
pub unsafe extern "C" fn hirari_vocal_remove_stereo_reference(
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) {
    if left.is_null() || right.is_null() || frames == 0 {
        return;
    }
    let mut bass_left = 0.0f32;
    let mut bass_right = 0.0f32;
    const COEFFICIENT: f32 = 0.02;
    for frame in 0..frames {
        let input_left = *left.add(frame);
        let input_right = *right.add(frame);
        bass_left += COEFFICIENT * (input_left - bass_left);
        bass_right += COEFFICIENT * (input_right - bass_right);
        let vocal = input_left - input_right;
        *left.add(frame) = vocal * 0.5 + bass_left * 0.5;
        *right.add(frame) = -vocal * 0.5 + bass_right * 0.5;
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_correlation_reference(
    left: *const f32,
    right: *const f32,
    frames: usize,
) -> f32 {
    if left.is_null() || right.is_null() || frames == 0 {
        return 1.0;
    }
    let mut sum_lr = 0.0f64;
    let mut sum_left = 0.0f64;
    let mut sum_right = 0.0f64;
    for frame in 0..frames {
        let l = *left.add(frame) as f64;
        let r = *right.add(frame) as f64;
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
