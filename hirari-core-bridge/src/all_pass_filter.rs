use std::ffi::c_void;

pub struct AllPassFilterEngine {
    pub delay_buffer: Vec<f32>,
    pub feedback: f32,
    pub idx: usize,
    pub mask: usize,
}

impl AllPassFilterEngine {
    pub fn new(delay_samples: usize, feedback: f32) -> Self {
        let size = delay_samples.saturating_add(1).next_power_of_two().max(1);

        Self {
            delay_buffer: vec![0.0; size],
            feedback,
            idx: 0,
            mask: size - 1,
        }
    }

    pub fn reset(&mut self) {
        self.delay_buffer.fill(0.0);
        self.idx = 0;
    }

    /// INDUSTRIAL: Phase-shifting without amplitude change for diffusion.
    pub fn process(&mut self, in_val: f32) -> f32 {
        let in_val = if in_val.is_finite() { in_val } else { 0.0 };
        let buffer_out = self.delay_buffer[self.idx];
        let out = -self.feedback * in_val + buffer_out;
        self.delay_buffer[self.idx] = in_val + self.feedback * buffer_out;

        self.idx = (self.idx + 1) & self.mask;
        if out.is_finite() {
            out
        } else {
            0.0
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide All Pass Filter state.
    pub fn audit_all_pass_filter(&self) -> bool {
        !self.delay_buffer.is_empty()
            && self.delay_buffer.len().is_power_of_two()
            && self.mask == self.delay_buffer.len() - 1
            && self.idx < self.delay_buffer.len()
            && self.feedback.is_finite()
            && self.feedback.abs() <= 1.0
            && self.delay_buffer.iter().all(|sample| sample.is_finite())
    }
}

#[no_mangle]
pub extern "C" fn hirari_all_pass_create(delay_samples: usize, feedback: f32) -> *mut c_void {
    Box::into_raw(Box::new(AllPassFilterEngine::new(delay_samples, feedback))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_all_pass_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: The handle is allocated by hirari_all_pass_create and destroyed once.
        drop(Box::from_raw(state.cast::<AllPassFilterEngine>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_all_pass_reset(state: *mut c_void) {
    if let Some(state) = state.cast::<AllPassFilterEngine>().as_mut() {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_all_pass_process(state: *mut c_void, input: f32) -> f32 {
    state.cast::<AllPassFilterEngine>().as_mut().map_or_else(
        || if input.is_finite() { input } else { 0.0 },
        |state| state.process(input),
    )
}

#[cfg(test)]
mod tests {
    use super::AllPassFilterEngine;

    #[test]
    fn all_pass_matches_native_equation_and_sanitizes_non_finite_input() {
        let mut engine = AllPassFilterEngine::new(2, 0.5);
        let inputs = [1.0, 0.0, f32::NAN, -0.25, 0.5, 0.0];
        let mut reference = vec![0.0f32; 4];
        let mut index = 0usize;
        for input in inputs {
            let safe_input = if input.is_finite() { input } else { 0.0 };
            let delayed = reference[index];
            let output = delayed - 0.5 * safe_input;
            reference[index] = safe_input + 0.5 * delayed;
            index = (index + 1) & 3;
            let actual = engine.process(input);
            assert!((actual - output).abs() < 1e-7);
        }
    }
}
