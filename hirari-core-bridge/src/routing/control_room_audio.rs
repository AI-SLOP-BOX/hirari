//! Allocation-free monitor and Cue mix loops used by the native realtime path.

const MAX_AUDIO_BLOCK: usize = 65_536;

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_process_monitor(
    left: *mut f32,
    right: *mut f32,
    talkback: *const f32,
    frames: u32,
    monitor_gain: f32,
    talkback_gain: f32,
    talkback_enabled: bool,
) {
    let frames = frames as usize;
    if left.is_null() || right.is_null() || frames == 0 || frames > MAX_AUDIO_BLOCK {
        return;
    }
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames) };
    let right = unsafe { std::slice::from_raw_parts_mut(right, frames) };
    let monitor_gain = monitor_gain.clamp(0.0, 4.0);
    for frame in 0..frames {
        let in_left = if left[frame].is_finite() {
            left[frame]
        } else {
            0.0
        };
        let in_right = if right[frame].is_finite() {
            right[frame]
        } else {
            0.0
        };
        left[frame] = (in_left * monitor_gain).clamp(-16.0, 16.0);
        right[frame] = (in_right * monitor_gain).clamp(-16.0, 16.0);
    }

    if talkback.is_null() || !talkback_enabled {
        return;
    }
    let talkback = unsafe { std::slice::from_raw_parts(talkback, frames) };
    let talkback_gain = talkback_gain.clamp(0.0, 4.0);
    for frame in 0..frames {
        let sample = if talkback[frame].is_finite() {
            talkback[frame].clamp(-16.0, 16.0) * talkback_gain
        } else {
            0.0
        };
        let monitor_left = if left[frame].is_finite() {
            left[frame]
        } else {
            0.0
        };
        let monitor_right = if right[frame].is_finite() {
            right[frame]
        } else {
            0.0
        };
        left[frame] = (monitor_left + sample).clamp(-16.0, 16.0);
        right[frame] = (monitor_right + sample).clamp(-16.0, 16.0);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_mix_cue(
    source_left: *const f32,
    source_right: *const f32,
    click_left: *const f32,
    click_right: *const f32,
    output_left: *mut f32,
    output_right: *mut f32,
    frames: u32,
    cue_gain: f32,
    click_enabled: bool,
) {
    let frames = frames as usize;
    if source_left.is_null()
        || source_right.is_null()
        || output_left.is_null()
        || output_right.is_null()
        || frames == 0
        || frames > MAX_AUDIO_BLOCK
        || (click_enabled && (click_left.is_null() || click_right.is_null()))
    {
        return;
    }
    let source_left = unsafe { std::slice::from_raw_parts(source_left, frames) };
    let source_right = unsafe { std::slice::from_raw_parts(source_right, frames) };
    let output_left = unsafe { std::slice::from_raw_parts_mut(output_left, frames) };
    let output_right = unsafe { std::slice::from_raw_parts_mut(output_right, frames) };
    let click_left = if click_enabled {
        Some(unsafe { std::slice::from_raw_parts(click_left, frames) })
    } else {
        None
    };
    let click_right = if click_enabled {
        Some(unsafe { std::slice::from_raw_parts(click_right, frames) })
    } else {
        None
    };
    for frame in 0..frames {
        let source_l = if source_left[frame].is_finite() {
            source_left[frame]
        } else {
            0.0
        };
        let source_r = if source_right[frame].is_finite() {
            source_right[frame]
        } else {
            0.0
        };
        let click_l = click_left.map_or(0.0, |click| {
            if click[frame].is_finite() {
                click[frame]
            } else {
                0.0
            }
        });
        let click_r = click_right.map_or(0.0, |click| {
            if click[frame].is_finite() {
                click[frame]
            } else {
                0.0
            }
        });
        output_left[frame] = (source_l * cue_gain + click_l).clamp(-16.0, 16.0);
        output_right[frame] = (source_r * cue_gain + click_r).clamp(-16.0, 16.0);
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "dsp-differential-reference")]
    unsafe extern "C" {
        fn hirari_control_room_process_monitor_reference(
            left: *mut f32,
            right: *mut f32,
            talkback: *const f32,
            frames: u32,
            monitor_gain: f32,
            talkback_gain: f32,
            talkback_enabled: bool,
        );
        fn hirari_control_room_mix_cue_reference(
            source_left: *const f32,
            source_right: *const f32,
            click_left: *const f32,
            click_right: *const f32,
            output_left: *mut f32,
            output_right: *mut f32,
            frames: u32,
            cue_gain: f32,
            click_enabled: bool,
        );
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn monitor_and_talkback_match_cpp_reference() {
        let mut rust_left = [0.5_f32, f32::NAN, 18.0, -18.0, -0.25];
        let mut rust_right = [-0.5_f32, 1.0, f32::INFINITY, 4.0, 0.25];
        let mut cpp_left = rust_left;
        let mut cpp_right = rust_right;
        let talkback = [0.1_f32, f32::NAN, 20.0, -20.0, 0.0];
        unsafe {
            super::hirari_control_room_process_monitor(
                rust_left.as_mut_ptr(),
                rust_right.as_mut_ptr(),
                talkback.as_ptr(),
                5,
                1.5,
                2.0,
                true,
            );
            hirari_control_room_process_monitor_reference(
                cpp_left.as_mut_ptr(),
                cpp_right.as_mut_ptr(),
                talkback.as_ptr(),
                5,
                1.5,
                2.0,
                true,
            );
        }
        assert_eq!(rust_left.map(f32::to_bits), cpp_left.map(f32::to_bits));
        assert_eq!(rust_right.map(f32::to_bits), cpp_right.map(f32::to_bits));
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn cue_bus_mix_with_and_without_click_matches_cpp_reference() {
        let source_left = [0.25_f32, f32::NAN, 18.0, -18.0];
        let source_right = [-0.5_f32, 1.0, f32::INFINITY, 4.0];
        let click_left = [0.1_f32, f32::NAN, 2.0, -2.0];
        let click_right = [-0.1_f32, 0.2, f32::NEG_INFINITY, 2.0];
        for enabled in [false, true] {
            let mut rust_left = [0.0_f32; 4];
            let mut rust_right = [0.0_f32; 4];
            let mut cpp_left = [0.0_f32; 4];
            let mut cpp_right = [0.0_f32; 4];
            unsafe {
                super::hirari_control_room_mix_cue(
                    source_left.as_ptr(),
                    source_right.as_ptr(),
                    click_left.as_ptr(),
                    click_right.as_ptr(),
                    rust_left.as_mut_ptr(),
                    rust_right.as_mut_ptr(),
                    4,
                    1.25,
                    enabled,
                );
                hirari_control_room_mix_cue_reference(
                    source_left.as_ptr(),
                    source_right.as_ptr(),
                    click_left.as_ptr(),
                    click_right.as_ptr(),
                    cpp_left.as_mut_ptr(),
                    cpp_right.as_mut_ptr(),
                    4,
                    1.25,
                    enabled,
                );
            }
            assert_eq!(rust_left.map(f32::to_bits), cpp_left.map(f32::to_bits));
            assert_eq!(rust_right.map(f32::to_bits), cpp_right.map(f32::to_bits));
        }
    }
}
