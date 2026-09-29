//! Per-Track channel EQ math used on the active audio render path.

fn gain_from_boost_cut(boost_db: f32, cut_db: f32) -> f32 {
    if !boost_db.is_finite() || !cut_db.is_finite() {
        return 1.0;
    }
    let db = (boost_db - cut_db).clamp(-120.0, 24.0);
    let gain = 10.0_f32.powf(db / 20.0);
    if gain.is_finite() {
        gain
    } else {
        1.0
    }
}

fn process_channel_eq(
    left: &mut [f32],
    right: &mut [f32],
    low_gain: f32,
    high_gain: f32,
    state_l: &mut f32,
    state_r: &mut f32,
) {
    let mut low_l = if state_l.is_finite() { *state_l } else { 0.0 };
    let mut low_r = if state_r.is_finite() { *state_r } else { 0.0 };
    let low_gain = if low_gain.is_finite() { low_gain } else { 1.0 };
    let high_gain = if high_gain.is_finite() {
        high_gain
    } else {
        1.0
    };

    for (sample_l, sample_r) in left.iter_mut().zip(right.iter_mut()) {
        let in_l = if sample_l.is_finite() {
            sample_l.clamp(-1.0e6, 1.0e6)
        } else {
            0.0
        };
        let in_r = if sample_r.is_finite() {
            sample_r.clamp(-1.0e6, 1.0e6)
        } else {
            0.0
        };
        low_l += 0.02 * (in_l - low_l);
        low_r += 0.02 * (in_r - low_r);
        let out_l = low_l * low_gain + (in_l - low_l) * high_gain;
        let out_r = low_r * low_gain + (in_r - low_r) * high_gain;
        if out_l.is_finite() {
            *sample_l = out_l;
        } else {
            *sample_l = 0.0;
            low_l = 0.0;
        }
        if out_r.is_finite() {
            *sample_r = out_r;
        } else {
            *sample_r = 0.0;
            low_r = 0.0;
        }
    }
    *state_l = low_l;
    *state_r = low_r;
}

#[no_mangle]
pub extern "C" fn hirari_track_eq_gain(boost_db: f32, cut_db: f32) -> f32 {
    gain_from_boost_cut(boost_db, cut_db)
}

/// # Safety
/// Non-null audio and state pointers must reference `frames` writable samples
/// and two writable state floats, respectively.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_eq_process(
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    low_gain: f32,
    high_gain: f32,
    state_l: *mut f32,
    state_r: *mut f32,
) {
    if frames == 0 || left.is_null() || right.is_null() || state_l.is_null() || state_r.is_null() {
        return;
    }
    unsafe {
        process_channel_eq(
            std::slice::from_raw_parts_mut(left, frames as usize),
            std::slice::from_raw_parts_mut(right, frames as usize),
            low_gain,
            high_gain,
            &mut *state_l,
            &mut *state_r,
        );
    }
}

#[cfg(all(test, feature = "dsp-differential-reference"))]
mod differential_tests {
    use super::*;

    unsafe extern "C" {
        fn hirari_track_eq_process_reference(
            left: *mut f32,
            right: *mut f32,
            frames: u32,
            low_gain: f32,
            high_gain: f32,
            state_l: *mut f32,
            state_r: *mut f32,
        );
        fn hirari_track_eq_gain_reference(boost_db: f32, cut_db: f32) -> f32;
    }

    #[test]
    fn gain_conversion_matches_the_previous_cpp_implementation() {
        for &(boost, cut) in &[
            (0.0, 0.0),
            (12.0, 3.0),
            (-24.0, 6.0),
            (120.0, -120.0),
            (-120.0, 120.0),
            (f32::NAN, 2.0),
            (3.0, f32::INFINITY),
        ] {
            let rust = gain_from_boost_cut(boost, cut);
            let cpp = unsafe { hirari_track_eq_gain_reference(boost, cut) };
            assert!(
                (rust - cpp).abs() <= 1.0e-6,
                "boost={boost}, cut={cut}: {rust} != {cpp}"
            );
        }
    }

    #[test]
    fn block_processing_matches_cpp_across_sizes_boundaries_and_bad_values() {
        let equivalent =
            |rust: f32, cpp: f32| (rust - cpp).abs() <= 2.0e-6 + 2.0e-6 * rust.abs().max(cpp.abs());
        let mut rust_state = [0.0, 0.0];
        let mut cpp_state = [0.0, 0.0];
        for (block_index, frames) in [1usize, 7, 64, 256, 1024, 13, 511].into_iter().enumerate() {
            let mut rust_l = Vec::with_capacity(frames);
            let mut rust_r = Vec::with_capacity(frames);
            for i in 0..frames {
                let n = (i + block_index * 31) as f32;
                rust_l.push(((n * 0.173).sin() * 2.0).clamp(-2.0, 2.0));
                rust_r.push(((n * 0.071).cos() * 4.0).clamp(-4.0, 4.0));
            }
            if block_index == 1 {
                rust_l[0] = f32::NAN;
                rust_r[1] = f32::INFINITY;
            }
            if block_index == 4 {
                rust_l[0] = 1.0e8;
                rust_r[0] = -1.0e8;
            }
            let mut cpp_l = rust_l.clone();
            let mut cpp_r = rust_r.clone();
            let low_gain = [1.0, 0.5, 2.0, 0.0][block_index % 4];
            let high_gain = [1.0, 2.0, 0.25, 8.0][block_index % 4];
            let (rust_state_l, rust_state_r) = rust_state.split_at_mut(1);
            let (cpp_state_l, cpp_state_r) = cpp_state.split_at_mut(1);
            process_channel_eq(
                &mut rust_l,
                &mut rust_r,
                low_gain,
                high_gain,
                &mut rust_state_l[0],
                &mut rust_state_r[0],
            );
            unsafe {
                hirari_track_eq_process_reference(
                    cpp_l.as_mut_ptr(),
                    cpp_r.as_mut_ptr(),
                    frames as u32,
                    low_gain,
                    high_gain,
                    &mut cpp_state_l[0],
                    &mut cpp_state_r[0],
                );
            }
            for (index, (&rust, &cpp)) in rust_l.iter().zip(&cpp_l).enumerate() {
                assert!(
                    equivalent(rust, cpp),
                    "left block={block_index} sample={index}: rust={rust} cpp={cpp} delta={}",
                    (rust - cpp).abs()
                );
            }
            for (index, (&rust, &cpp)) in rust_r.iter().zip(&cpp_r).enumerate() {
                assert!(
                    equivalent(rust, cpp),
                    "right block={block_index} sample={index}: rust={rust} cpp={cpp} delta={}",
                    (rust - cpp).abs()
                );
            }
            assert!((rust_state[0] - cpp_state[0]).abs() <= 1.0e-6);
            assert!((rust_state[1] - cpp_state[1]).abs() <= 1.0e-6);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_gain_inputs_are_neutral_and_extremes_are_bounded() {
        assert_eq!(gain_from_boost_cut(f32::NAN, 0.0), 1.0);
        assert!((gain_from_boost_cut(120.0, -120.0) - 10.0_f32.powf(1.2)).abs() < 1e-6);
        assert!((gain_from_boost_cut(-120.0, 120.0) - 1.0e-6).abs() < 1e-8);
    }
}
