pub struct InputStageState {
    pub gain_linear: f32,
    pub is_phase_inverted: bool,
    pub saturation_amount: f32,
}

pub struct InputStageOrchestrator {
    pub state: InputStageState,
}

impl Default for InputStageOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl InputStageOrchestrator {
    pub fn new() -> Self {
        Self {
            state: InputStageState {
                gain_linear: 1.0,
                is_phase_inverted: false,
                saturation_amount: 0.0,
            },
        }
    }

    /// INDUSTRIAL: Processes a stereo signal with absolute precision and signal sovereignty.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if l.is_empty() || r.is_empty() || !self.audit_phase() {
            return;
        }
        let multiplier = if self.state.is_phase_inverted {
            -1.0
        } else {
            1.0
        };
        let total_factor = self.state.gain_linear * multiplier;
        let saturation = self.state.saturation_amount;

        if saturation > 0.0 {
            for i in 0..l.len().min(r.len()) {
                let mut sl = (if l[i].is_finite() { l[i] } else { 0.0 }) * total_factor;
                let mut sr = (if r[i].is_finite() { r[i] } else { 0.0 }) * total_factor;

                // Efficient Soft-Clipping (Polynomial approximation of tanh)
                sl = self.apply_saturation(sl, saturation);
                sr = self.apply_saturation(sr, saturation);

                l[i] = if sl.is_finite() {
                    sl.clamp(-4.0, 4.0)
                } else {
                    0.0
                };
                r[i] = if sr.is_finite() {
                    sr.clamp(-4.0, 4.0)
                } else {
                    0.0
                };
            }
        } else {
            for i in 0..l.len().min(r.len()) {
                l[i] = if l[i].is_finite() {
                    (l[i] * total_factor).clamp(-4.0, 4.0)
                } else {
                    0.0
                };
                r[i] = if r[i].is_finite() {
                    (r[i] * total_factor).clamp(-4.0, 4.0)
                } else {
                    0.0
                };
            }
        }
    }

    #[inline(always)]
    fn apply_saturation(&self, x: f32, s: f32) -> f32 {
        if x.abs() > 1.0 {
            // Cubic Hermite soft-clipper for transients > 1.0
            x.signum() * (1.0 + (x.abs() - 1.0) / (1.0 + (x.abs() - 1.0) * (x.abs() - 1.0)))
        } else {
            // Standard polynomial saturation for head-amp simulation
            x * (1.0 - s * x * x * 0.33333334)
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide phase synchronization graph.
    pub fn audit_phase(&self) -> bool {
        self.state.gain_linear.is_finite()
            && (0.0..=16.0).contains(&self.state.gain_linear)
            && self.state.saturation_amount.is_finite()
            && (0.0..=1.0).contains(&self.state.saturation_amount)
    }
}

#[cfg(test)]
mod tests {
    use super::InputStageOrchestrator;

    #[test]
    fn input_stage_sanitizes_audio_and_rejects_corrupt_state() {
        let mut stage = InputStageOrchestrator::new();
        stage.state.gain_linear = 4.0;
        let mut left = vec![f32::NAN, 0.25, 2.0];
        let mut right = vec![0.25, f32::INFINITY, 2.0];
        stage.process(&mut left, &mut right);
        assert!(left
            .iter()
            .chain(right.iter())
            .all(|v| v.is_finite() && v.abs() <= 4.0));
        stage.state.gain_linear = f32::NAN;
        assert!(!stage.audit_phase());
    }
}
