/// Stateful two-branch all-pass 2x interpolator used by AnalogSaturator.
/// This matches the former C++ Oversampler2x recurrence exactly.
pub struct OversamplerEngine {
    pub phase_state_1: f32,
    pub phase_state_2: f32,
}

impl Default for OversamplerEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl OversamplerEngine {
    pub const A1: f32 = 0.129_676_545_786_47;
    pub const A2: f32 = 0.484_189_234_343_41;

    pub fn new() -> Self {
        Self {
            phase_state_1: 0.0,
            phase_state_2: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.phase_state_1 = 0.0;
        self.phase_state_2 = 0.0;
    }

    pub fn upsample(&mut self, input: f32) -> (f32, f32) {
        let input = if input.is_finite() { input } else { 0.0 };
        let first = Self::A1 * (input - self.phase_state_1) + self.phase_state_1;
        self.phase_state_1 = first;
        let second = Self::A2 * (input - self.phase_state_2) + self.phase_state_2;
        self.phase_state_2 = second;
        (first, second)
    }

    pub fn downsample(first: f32, second: f32) -> f32 {
        let first = if first.is_finite() { first } else { 0.0 };
        let second = if second.is_finite() { second } else { 0.0 };
        (first + second) * 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::OversamplerEngine;

    #[test]
    fn all_pass_interpolation_matches_native_recurrence() {
        let mut rust = OversamplerEngine::new();
        let mut state_1 = 0.0f32;
        let mut state_2 = 0.0f32;
        for input in [0.0, 1.0, -0.5, 0.25, 0.0, f32::NAN, 0.75] {
            let safe_input = if input.is_finite() { input } else { 0.0 };
            let expected_1 = OversamplerEngine::A1 * (safe_input - state_1) + state_1;
            state_1 = expected_1;
            let expected_2 = OversamplerEngine::A2 * (safe_input - state_2) + state_2;
            state_2 = expected_2;
            let (actual_1, actual_2) = rust.upsample(input);
            assert_eq!(actual_1, expected_1);
            assert_eq!(actual_2, expected_2);
        }
    }
}
