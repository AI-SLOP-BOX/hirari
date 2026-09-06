pub struct MasterSuiteEngine {
    pub sample_rate: f64,
    pub auto_gain_offset: f32,
}

impl MasterSuiteEngine {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            auto_gain_offset: 0.0,
        }
    }

    /// INDUSTRIAL: Applies AI-driven automatic gain staging.
    pub fn apply_ai_gain(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        current_lufs: f32,
        target_lufs: f32,
    ) {
        if current_lufs.is_finite()
            && target_lufs.is_finite()
            && current_lufs > -60.0
            && self.auto_gain_offset.is_finite()
        {
            let diff = (target_lufs - current_lufs).clamp(-24.0, 24.0);
            // Slow pursuit (0.005) for transparent gain adjustment
            self.auto_gain_offset =
                (self.auto_gain_offset + (diff - self.auto_gain_offset) * 0.005).clamp(-24.0, 24.0);
            if !self.auto_gain_offset.is_finite() {
                return;
            }
            let gain_factor = 10.0f32.powf(self.auto_gain_offset / 20.0);
            if !gain_factor.is_finite() {
                return;
            }

            for i in 0..l.len().min(r.len()) {
                let left = if l[i].is_finite() { l[i] } else { 0.0 };
                let right = if r[i].is_finite() { r[i] } else { 0.0 };
                l[i] = (left * gain_factor).clamp(-1.0, 1.0);
                r[i] = (right * gain_factor).clamp(-1.0, 1.0);
            }
        }
    }

    /// INDUSTRIAL: Processes Mid/Side stereo width.
    pub fn process_mid_side(&self, l: &mut [f32], r: &mut [f32], stereo_width: f32) {
        if !stereo_width.is_finite() {
            return;
        }
        let stereo_width = stereo_width.clamp(0.0, 2.0);
        let num_samples = l.len().min(r.len());

        for i in 0..num_samples {
            let left = if l[i].is_finite() {
                l[i].clamp(-1.0, 1.0)
            } else {
                0.0
            };
            let right = if r[i].is_finite() {
                r[i].clamp(-1.0, 1.0)
            } else {
                0.0
            };

            // Encode: L/R -> M/S
            let mid = (left + right) * 0.5;
            let mut side = (left - right) * 0.5;

            // Scale Side channel for Width
            side *= stereo_width;

            // Decode: M/S -> L/R
            l[i] = (mid + side).clamp(-1.0, 1.0);
            r[i] = (mid - side).clamp(-1.0, 1.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Master Suite state.
    pub fn audit_master_suite(&self) -> bool {
        // A usable engine needs a real, finite audio clock and a bounded gain
        // correction.  Zero is also rejected: it represents an uninitialized
        // (empty) engine state and would make any rate-dependent processing
        // invalid.
        self.sample_rate.is_finite()
            && (1.0..=768_000.0).contains(&self.sample_rate)
            && self.auto_gain_offset.is_finite()
            && (-120.0..=120.0).contains(&self.auto_gain_offset)
    }
}

#[cfg(test)]
mod tests {
    use super::MasterSuiteEngine;

    #[test]
    fn master_suite_processes_only_finite_bounded_audio() {
        let mut engine = MasterSuiteEngine::new(48_000.0);
        let mut left = vec![f32::NAN, 2.0, -2.0, 0.25];
        let mut right = vec![f32::INFINITY, -2.0, 2.0, -0.25];
        engine.apply_ai_gain(&mut left, &mut right, -12.0, -9.0);
        engine.process_mid_side(&mut left, &mut right, 1.5);
        assert!(left
            .iter()
            .chain(right.iter())
            .all(|sample| sample.is_finite() && sample.abs() <= 1.0));
    }
}
