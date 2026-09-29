use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

pub struct StereoExpanderEngine {
    width: AtomicU32,
    mid_gain: AtomicU32,
}

impl Default for StereoExpanderEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl StereoExpanderEngine {
    pub fn new() -> Self {
        Self {
            width: AtomicU32::new(1.0f32.to_bits()),
            mid_gain: AtomicU32::new(1.0f32.to_bits()),
        }
    }

    pub fn set_params(&self, width: f32, mid_gain: f32) {
        if width.is_finite() && mid_gain.is_finite() {
            self.width
                .store(width.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
            self.mid_gain
                .store(mid_gain.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
        }
    }

    fn parameters(&self) -> (f32, f32) {
        (
            f32::from_bits(self.width.load(Ordering::Relaxed)),
            f32::from_bits(self.mid_gain.load(Ordering::Relaxed)),
        )
    }

    /// Mid/side matrixing with the C++ processor's wet/dry blend.
    pub fn process(&self, left: &mut [f32], right: &mut [f32], mix: f32) {
        let (width, mid_gain) = self.parameters();
        let mix = if mix.is_finite() {
            mix.clamp(0.0, 1.0)
        } else {
            0.0
        };
        for (left, right) in left.iter_mut().zip(right) {
            let dry_left = *left;
            let dry_right = *right;
            let mid = 0.5 * (dry_left + dry_right) * mid_gain;
            let side = 0.5 * (dry_left - dry_right) * width;
            let wet_left = mid + side;
            let wet_right = mid - side;
            *left = dry_left + mix * (wet_left - dry_left);
            *right = dry_right + mix * (wet_right - dry_right);
        }
    }

    pub fn audit_stereo_expander(&self) -> bool {
        let (width, mid_gain) = self.parameters();
        width.is_finite()
            && (0.0..=2.0).contains(&width)
            && mid_gain.is_finite()
            && (0.0..=2.0).contains(&mid_gain)
    }
}

#[no_mangle]
pub extern "C" fn hirari_stereo_expander_create() -> *mut c_void {
    Box::into_raw(Box::new(StereoExpanderEngine::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_expander_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: handle was allocated by `hirari_stereo_expander_create`.
        unsafe { drop(Box::from_raw(state.cast::<StereoExpanderEngine>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_expander_set_params(
    state: *mut c_void,
    width: f32,
    mid_gain: f32,
) {
    if let Some(state) = unsafe { state.cast::<StereoExpanderEngine>().as_ref() } {
        state.set_params(width, mid_gain);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_expander_set_parameter(
    state: *mut c_void,
    parameter: u32,
    value: f32,
) {
    let Some(state) = (unsafe { state.cast::<StereoExpanderEngine>().as_ref() }) else {
        return;
    };
    if !value.is_finite() {
        return;
    }
    let value = value.clamp(0.0, 2.0).to_bits();
    match parameter {
        0 => state.width.store(value, Ordering::Relaxed),
        1 => state.mid_gain.store(value, Ordering::Relaxed),
        _ => {}
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_expander_get_parameter(
    state: *const c_void,
    parameter: u32,
) -> f32 {
    let Some(state) = (unsafe { state.cast::<StereoExpanderEngine>().as_ref() }) else {
        return 0.0;
    };
    let (width, mid_gain) = state.parameters();
    match parameter {
        0 => width,
        1 => mid_gain,
        _ => 0.0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_expander_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    mix: f32,
) {
    let Some(state) = (unsafe { state.cast::<StereoExpanderEngine>().as_ref() }) else {
        return;
    };
    if left.is_null() || right.is_null() || left == right || frames == 0 {
        return;
    }
    // SAFETY: caller supplies disjoint left/right planes with `frames` samples.
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames as usize) };
    let right = unsafe { std::slice::from_raw_parts_mut(right, frames as usize) };
    state.process(left, right, mix);
}

#[cfg(test)]
mod tests {
    use super::StereoExpanderEngine;

    #[test]
    fn stereo_expander_matches_mid_side_and_wet_dry_formula() {
        let engine = StereoExpanderEngine::new();
        engine.set_params(1.5, 0.75);
        let mut left = [0.8, -0.4, 0.1];
        let mut right = [0.2, 0.5, -0.9];
        let original_left = left;
        let original_right = right;
        let mix = 0.35;
        let mut expected_left = original_left;
        let mut expected_right = original_right;
        for index in 0..left.len() {
            let mid = 0.5 * (original_left[index] + original_right[index]) * 0.75;
            let side = 0.5 * (original_left[index] - original_right[index]) * 1.5;
            expected_left[index] += mix * (mid + side - original_left[index]);
            expected_right[index] += mix * (mid - side - original_right[index]);
        }
        engine.process(&mut left, &mut right, mix);
        assert_eq!(left, expected_left);
        assert_eq!(right, expected_right);
        assert!(engine.audit_stereo_expander());
    }

    #[test]
    fn parameters_clamp_and_non_finite_updates_are_ignored() {
        let engine = StereoExpanderEngine::new();
        engine.set_params(8.0, 0.0);
        assert_eq!(engine.parameters(), (2.0, 0.0));
        engine.set_params(f32::NAN, 1.0);
        assert_eq!(engine.parameters(), (2.0, 0.0));
    }
}
