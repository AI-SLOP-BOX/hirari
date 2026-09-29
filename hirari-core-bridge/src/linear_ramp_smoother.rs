use std::ffi::c_void;

/// Rust implementation of the legacy DSP linear-ramp smoother contract.
#[derive(Clone)]
pub struct LinearRampSmoother {
    current: f32,
    target: f32,
    start: f32,
    increment: f32,
    step_samples: u32,
    remaining: u32,
    sample_rate: f64,
}

impl Default for LinearRampSmoother {
    fn default() -> Self {
        Self {
            current: 0.0,
            target: 0.0,
            start: 0.0,
            increment: 0.0,
            step_samples: 441,
            remaining: 0,
            sample_rate: 44_100.0,
        }
    }
}

impl LinearRampSmoother {
    fn reset(&mut self, sample_rate: f64, time_ms: f64) {
        self.sample_rate = sample_rate;
        self.step_samples = (sample_rate * (time_ms / 1000.0)) as u32;
        self.remaining = 0;
    }

    fn set_target(&mut self, target: f32) {
        if (target - self.target).abs() < 1.0e-6 {
            return;
        }
        self.target = target;
        self.start = self.current;
        self.increment = (self.target - self.start) / self.step_samples.max(1) as f32;
        self.remaining = self.step_samples;
    }

    fn next_value(&mut self) -> f32 {
        if self.remaining > 0 {
            self.current += self.increment;
            self.remaining -= 1;
        } else {
            self.current = self.target;
        }
        self.current
    }

    fn skip(&mut self, samples: u32) {
        for _ in 0..samples {
            self.next_value();
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_linear_ramp_smoother_create() -> *mut c_void {
    Box::into_raw(Box::new(LinearRampSmoother::default())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_linear_ramp_smoother_clone(state: *const c_void) -> *mut c_void {
    unsafe { state.cast::<LinearRampSmoother>().as_ref() }.map_or(std::ptr::null_mut(), |state| {
        Box::into_raw(Box::new(state.clone())).cast()
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_linear_ramp_smoother_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: the handle is allocated by the matching constructor and destroyed once.
        unsafe { drop(Box::from_raw(state.cast::<LinearRampSmoother>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_linear_ramp_smoother_reset(
    state: *mut c_void,
    sample_rate: f64,
    time_ms: f64,
) {
    if let Some(state) = unsafe { state.cast::<LinearRampSmoother>().as_mut() } {
        state.reset(sample_rate, time_ms);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_linear_ramp_smoother_set_target(state: *mut c_void, target: f32) {
    if let Some(state) = unsafe { state.cast::<LinearRampSmoother>().as_mut() } {
        state.set_target(target);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_linear_ramp_smoother_next(state: *mut c_void) -> f32 {
    unsafe { state.cast::<LinearRampSmoother>().as_mut() }
        .map_or(0.0, LinearRampSmoother::next_value)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_linear_ramp_smoother_skip(state: *mut c_void, samples: u32) {
    if let Some(state) = unsafe { state.cast::<LinearRampSmoother>().as_mut() } {
        state.skip(samples);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_linear_ramp_smoother_current(state: *const c_void) -> f32 {
    unsafe { state.cast::<LinearRampSmoother>().as_ref() }.map_or(0.0, |state| state.current)
}

#[cfg(test)]
mod tests {
    use super::LinearRampSmoother;

    #[test]
    fn preserves_linear_ramp_and_skip_semantics() {
        let mut smoother = LinearRampSmoother::default();
        smoother.reset(1_000.0, 4.0);
        smoother.set_target(1.0);
        assert_eq!(smoother.next_value(), 0.25);
        smoother.skip(2);
        assert_eq!(smoother.next_value(), 1.0);
        assert_eq!(smoother.current, 1.0);
    }

    #[test]
    fn a_zero_length_ramp_reaches_target_on_the_next_sample() {
        let mut smoother = LinearRampSmoother::default();
        smoother.reset(48_000.0, 0.0);
        smoother.set_target(-0.5);
        assert_eq!(smoother.next_value(), -0.5);
    }

    #[test]
    fn cloned_state_keeps_an_independent_ramp_position() {
        let mut original = LinearRampSmoother::default();
        original.reset(1_000.0, 4.0);
        original.set_target(1.0);
        assert_eq!(original.next_value(), 0.25);
        let mut copied = original.clone();
        assert_eq!(copied.next_value(), 0.5);
        assert_eq!(original.current, 0.25);
    }
}
