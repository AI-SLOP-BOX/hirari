//! Allocation-free post-routing master output stage.
//!
//! This preserves the engine's order: sanitize/clamp click and add it, sanitize
//! the summed master signal, then apply the master gain before limiting.

const MAX_AUDIO_BLOCK: usize = 65_536;

#[no_mangle]
pub unsafe extern "C" fn hirari_master_output_process(
    left: *mut f32,
    right: *mut f32,
    click_left: *mut f32,
    click_right: *mut f32,
    frames: u32,
    master_gain: f32,
    include_click: bool,
) {
    let frames = frames as usize;
    if left.is_null()
        || right.is_null()
        || frames == 0
        || frames > MAX_AUDIO_BLOCK
        || (include_click && (click_left.is_null() || click_right.is_null()))
    {
        return;
    }
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames) };
    let right = unsafe { std::slice::from_raw_parts_mut(right, frames) };

    if include_click {
        let click_left = unsafe { std::slice::from_raw_parts_mut(click_left, frames) };
        let click_right = unsafe { std::slice::from_raw_parts_mut(click_right, frames) };
        for frame in 0..frames {
            let click_l = if click_left[frame].is_finite() {
                click_left[frame]
            } else {
                0.0
            };
            let click_r = if click_right[frame].is_finite() {
                click_right[frame]
            } else {
                0.0
            };
            click_left[frame] = click_l;
            click_right[frame] = click_r;
            let main_l = if left[frame].is_finite() {
                left[frame]
            } else {
                0.0
            };
            let main_r = if right[frame].is_finite() {
                right[frame]
            } else {
                0.0
            };
            left[frame] = (main_l + click_l).clamp(-16.0, 16.0);
            right[frame] = (main_r + click_r).clamp(-16.0, 16.0);
        }
    }

    for frame in 0..frames {
        if !left[frame].is_finite() {
            left[frame] = 0.0;
        }
        if !right[frame].is_finite() {
            right[frame] = 0.0;
        }
        left[frame] *= master_gain;
        right[frame] *= master_gain;
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "dsp-differential-reference")]
    unsafe extern "C" {
        fn hirari_master_output_reference_process(
            left: *mut f32,
            right: *mut f32,
            click_left: *mut f32,
            click_right: *mut f32,
            frames: u32,
            master_gain: f32,
            include_click: bool,
        );
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn master_output_stage_matches_cpp_before_limiter() {
        let original_left = [0.25_f32, f32::NAN, 18.0, -18.0, 0.0, 1.25];
        let original_right = [-0.5_f32, 0.75, f32::INFINITY, -4.0, 16.0, -1.25];
        let original_click_left = [0.1_f32, f32::NAN, 2.0, -2.0, 0.5, 0.0];
        let original_click_right = [-0.1_f32, 0.2, f32::NEG_INFINITY, 2.0, -0.5, 0.0];
        for include_click in [false, true] {
            let mut rust_left = original_left;
            let mut rust_right = original_right;
            let mut rust_click_left = original_click_left;
            let mut rust_click_right = original_click_right;
            let mut cpp_left = original_left;
            let mut cpp_right = original_right;
            let mut cpp_click_left = original_click_left;
            let mut cpp_click_right = original_click_right;

            unsafe {
                super::hirari_master_output_process(
                    rust_left.as_mut_ptr(),
                    rust_right.as_mut_ptr(),
                    rust_click_left.as_mut_ptr(),
                    rust_click_right.as_mut_ptr(),
                    rust_left.len() as u32,
                    0.875,
                    include_click,
                );
                hirari_master_output_reference_process(
                    cpp_left.as_mut_ptr(),
                    cpp_right.as_mut_ptr(),
                    cpp_click_left.as_mut_ptr(),
                    cpp_click_right.as_mut_ptr(),
                    cpp_left.len() as u32,
                    0.875,
                    include_click,
                );
            }
            assert_eq!(rust_left.map(f32::to_bits), cpp_left.map(f32::to_bits));
            assert_eq!(rust_right.map(f32::to_bits), cpp_right.map(f32::to_bits));
            assert_eq!(
                rust_click_left.map(f32::to_bits),
                cpp_click_left.map(f32::to_bits)
            );
            assert_eq!(
                rust_click_right.map(f32::to_bits),
                cpp_click_right.map(f32::to_bits)
            );
        }
    }
}
