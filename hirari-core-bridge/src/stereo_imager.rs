use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

struct StereoImagerRuntime {
    sample_rate: f64,
    current_side_gain: f32,
}

/// Rust owns the real-time mid/side processing state. Width is atomic so UI
/// automation can update the target while the audio thread smooths toward it.
pub struct StereoImagerEngine {
    width: AtomicU32,
    runtime: UnsafeCell<StereoImagerRuntime>,
}

impl StereoImagerEngine {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            width: AtomicU32::new(1.0f32.to_bits()),
            runtime: UnsafeCell::new(StereoImagerRuntime {
                sample_rate: valid_sample_rate(sample_rate),
                current_side_gain: 1.0,
            }),
        }
    }

    pub fn reset(&self) {
        // SAFETY: reset is serialized with audio processing by the host.
        unsafe { &mut *self.runtime.get() }.current_side_gain = self.width();
    }

    pub fn set_sample_rate(&self, sample_rate: f64) {
        // SAFETY: prepare/reset is serialized with audio processing by the host.
        unsafe { &mut *self.runtime.get() }.sample_rate = valid_sample_rate(sample_rate);
    }

    pub fn width(&self) -> f32 {
        f32::from_bits(self.width.load(Ordering::Relaxed))
    }

    pub fn set_width(&self, width: f32) {
        let width = if width.is_finite() {
            width.clamp(0.0, 4.0)
        } else {
            1.0
        };
        self.width.store(width.to_bits(), Ordering::Relaxed);
    }

    pub fn process(&self, left: &mut [f32], right: &mut [f32], mix: f32) {
        let runtime = unsafe { &mut *self.runtime.get() };
        if !runtime.sample_rate.is_finite() || runtime.sample_rate <= 0.0 {
            return;
        }
        let len = left.len().min(right.len());
        let target = self.width();
        let mix = if mix.is_finite() {
            mix.clamp(0.0, 1.0)
        } else {
            0.0
        };
        for index in 0..len {
            runtime.current_side_gain += (target - runtime.current_side_gain) * 0.01;
            let mid = 0.5 * (left[index] + right[index]);
            let side = 0.5 * (left[index] - right[index]) * runtime.current_side_gain;
            let wet_left = mid + side;
            let wet_right = mid - side;
            left[index] = left[index] * (1.0 - mix) + wet_left * mix;
            right[index] = right[index] * (1.0 - mix) + wet_right * mix;
        }
    }
}

fn valid_sample_rate(sample_rate: f64) -> f64 {
    if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
        sample_rate
    } else {
        44_100.0
    }
}

#[no_mangle]
pub extern "C" fn hirari_stereo_imager_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(StereoImagerEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_imager_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: the handle was allocated by `hirari_stereo_imager_create`.
        drop(unsafe { Box::from_raw(state.cast::<StereoImagerEngine>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_imager_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<StereoImagerEngine>().as_ref() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_imager_set_sample_rate(
    state: *mut c_void,
    sample_rate: f64,
) {
    if let Some(state) = unsafe { state.cast::<StereoImagerEngine>().as_ref() } {
        state.set_sample_rate(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_imager_set_width(state: *mut c_void, width: f32) {
    if let Some(state) = unsafe { state.cast::<StereoImagerEngine>().as_ref() } {
        state.set_width(width);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_imager_get_width(state: *const c_void) -> f32 {
    unsafe { state.cast::<StereoImagerEngine>().as_ref() }.map_or(1.0, |state| state.width())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_imager_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    mix: f32,
) {
    let Some(state) = (unsafe { state.cast::<StereoImagerEngine>().as_ref() }) else {
        return;
    };
    if left.is_null() || right.is_null() || right == left || frames == 0 {
        return;
    }
    // SAFETY: caller supplies disjoint left/right planes with `frames` samples.
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames as usize) };
    let right = unsafe { std::slice::from_raw_parts_mut(right, frames as usize) };
    state.process(left, right, mix);
}

#[cfg(test)]
mod tests {
    use super::StereoImagerEngine;

    #[test]
    fn width_smoothing_and_mid_side_output_match_native_formula() {
        let engine = StereoImagerEngine::new(48_000.0);
        engine.set_width(2.0);
        let mut left = [0.8, -0.25, 0.5];
        let mut right = [0.2, 0.75, -0.5];
        let (mut gain, target) = (1.0f32, 2.0f32);
        for index in 0..left.len() {
            gain += (target - gain) * 0.01;
            let input_left = [0.8, -0.25, 0.5][index];
            let input_right = [0.2, 0.75, -0.5][index];
            let mid = 0.5 * (input_left + input_right);
            let side = 0.5 * (input_left - input_right) * gain;
            left[index] = mid + side;
            right[index] = mid - side;
        }
        let mut actual_left = [0.8, -0.25, 0.5];
        let mut actual_right = [0.2, 0.75, -0.5];
        engine.process(&mut actual_left, &mut actual_right, 1.0);
        assert_eq!(actual_left, left);
        assert_eq!(actual_right, right);
    }

    #[test]
    fn width_is_clamped_and_invalid_sample_rate_falls_back() {
        let engine = StereoImagerEngine::new(f64::NAN);
        engine.set_width(f32::INFINITY);
        assert_eq!(engine.width(), 1.0);
        engine.set_width(9.0);
        assert_eq!(engine.width(), 4.0);
    }
}
