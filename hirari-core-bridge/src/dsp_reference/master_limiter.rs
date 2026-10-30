use std::ffi::c_void;

const MAX_LOOKAHEAD: usize = 2048;
const MASK: usize = MAX_LOOKAHEAD - 1;

struct MasterLimiter {
    sample_rate: f64,
    delay_left: Box<[f32; MAX_LOOKAHEAD]>,
    delay_right: Box<[f32; MAX_LOOKAHEAD]>,
    write_index: usize,
    current_gain: f32,
}

#[no_mangle]
pub extern "C" fn hirari_master_limiter_reference_create(sample_rate: f64) -> *mut c_void {
    let sample_rate = if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
        sample_rate
    } else {
        44_100.0
    };
    Box::into_raw(Box::new(MasterLimiter {
        sample_rate,
        delay_left: Box::new([0.0; MAX_LOOKAHEAD]),
        delay_right: Box::new([0.0; MAX_LOOKAHEAD]),
        write_index: 0,
        current_gain: 1.0,
    })) as *mut c_void
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_limiter_reference_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state as *mut MasterLimiter));
    }
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn hirari_master_limiter_reference_process(
    opaque: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
    has_right: bool,
    threshold_gain: f32,
    ceiling: f32,
    release_ms: f32,
    lookahead_ms: f32,
) {
    if opaque.is_null() || left.is_null() || (has_right && right.is_null()) {
        return;
    }
    let state = &mut *(opaque as *mut MasterLimiter);
    let sample_rate = if state.sample_rate > 0.0 {
        state.sample_rate
    } else {
        44_100.0
    };
    let release_ms = if release_ms.is_finite() {
        release_ms.clamp(1.0, 1000.0)
    } else {
        50.0
    };
    let mut release_coefficient =
        (-1.0f32 / (release_ms * 0.001 * sample_rate as f32)).exp();
    if !release_coefficient.is_finite() {
        release_coefficient = 0.99;
    }
    release_coefficient = release_coefficient.clamp(0.0, 0.9999);
    let threshold_gain = if threshold_gain.is_finite() {
        threshold_gain.clamp(0.001, 15.8489)
    } else {
        1.0
    };
    let ceiling = if ceiling.is_finite() {
        ceiling.clamp(0.001, 1.0)
    } else {
        0.99
    };
    let lookahead_ms = if lookahead_ms.is_finite() {
        lookahead_ms.clamp(0.0, 20.0)
    } else {
        2.0
    };
    let lookahead_samples = ((lookahead_ms * 0.001 * sample_rate as f32).round() as u32)
        .clamp(0, MAX_LOOKAHEAD as u32 - 1) as usize;

    for frame in 0..frames {
        let input_left = {
            let value = *left.add(frame);
            if value.is_finite() {
                value * threshold_gain
            } else {
                0.0
            }
        };
        let input_right = if has_right {
            let value = *right.add(frame);
            if value.is_finite() {
                value * threshold_gain
            } else {
                input_left
            }
        } else {
            input_left
        };
        state.delay_left[state.write_index] = input_left;
        state.delay_right[state.write_index] = input_right;
        let read_index = (state.write_index + MAX_LOOKAHEAD - lookahead_samples) & MASK;
        let output_left = state.delay_left[read_index];
        let output_right = state.delay_right[read_index];

        let mut peak = 0.0f32;
        let window = lookahead_samples + 1;
        for ahead in 0..window {
            let index = (read_index + ahead) & MASK;
            let previous = (index + MAX_LOOKAHEAD - 1) & MASK;
            peak = peak.max(state.delay_left[index].abs().max(state.delay_right[index].abs()));
            peak = peak.max(
                (0.5 * (state.delay_left[previous] + state.delay_left[index]))
                    .abs()
                    .max((0.5 * (state.delay_right[previous] + state.delay_right[index])).abs()),
            );
        }
        state.write_index = (state.write_index + 1) & MASK;
        let mut target_attenuation = if peak > ceiling {
            ceiling / (peak + 1.0e-6)
        } else {
            1.0
        };
        if !target_attenuation.is_finite() {
            target_attenuation = 1.0;
        }
        if target_attenuation < state.current_gain {
            state.current_gain = target_attenuation;
        } else {
            state.current_gain = state.current_gain * release_coefficient
                + target_attenuation * (1.0 - release_coefficient);
        }
        if !state.current_gain.is_finite() {
            state.current_gain = 1.0;
        }

        let limited_left = output_left * state.current_gain;
        *left.add(frame) = if limited_left.is_finite() {
            limited_left.clamp(-ceiling, ceiling)
        } else {
            0.0
        };
        if has_right {
            let limited_right = output_right * state.current_gain;
            *right.add(frame) = if limited_right.is_finite() {
                limited_right.clamp(-ceiling, ceiling)
            } else {
                0.0
            };
        }
    }
}
