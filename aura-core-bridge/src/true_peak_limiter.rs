use crate::delay_line::DelayLineEngine;

pub struct TruePeakLimiterEngine {
    pub sample_rate: f64,
    pub delay_samples: u32,
    pub delay_l: DelayLineEngine,
    pub delay_r: DelayLineEngine,
    pub current_gain: f32,
    pub z2_l: f32,
    pub z2_r: f32,
    pub z1_l: f32,
    pub z1_r: f32,
}

impl TruePeakLimiterEngine {
    pub fn new(sr: f64) -> Self {
        let sr = if sr.is_finite() && sr >= 8_000.0 {
            sr
        } else {
            48_000.0
        };
        let delay_samples = (sr * 0.0015).round().clamp(1.0, u32::MAX as f64) as u32; // 1.5ms
        Self {
            sample_rate: sr,
            delay_samples,
            delay_l: DelayLineEngine::new(delay_samples + 1),
            delay_r: DelayLineEngine::new(delay_samples + 1),
            current_gain: 1.0,
            z2_l: 0.0,
            z2_r: 0.0,
            z1_l: 0.0,
            z1_r: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.current_gain = 1.0;
        self.z2_l = 0.0;
        self.z2_r = 0.0;
        self.delay_l.reset();
        self.delay_r.reset();
        self.z1_l = 0.0;
        self.z1_r = 0.0;
    }

    /// INDUSTRIAL: Professional Mastering-Grade Brickwall Limiter with ISP Detection.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], threshold_db: f32, ceiling_db: f32) {
        let len = l.len().min(r.len());
        if len == 0 {
            return;
        }
        let threshold_db = if threshold_db.is_finite() {
            threshold_db
        } else {
            0.0
        };
        let ceiling_db = if ceiling_db.is_finite() {
            ceiling_db
        } else {
            -1.0
        };
        let ceiling = 10.0f32.powf(ceiling_db / 20.0).clamp(1.0e-6, 1.0);
        let threshold = 10.0f32.powf(threshold_db / 20.0).clamp(1.0e-6, ceiling);

        for s in 0..len {
            let in_l = if l[s].is_finite() { l[s] } else { 0.0 };
            let in_r = if r[s].is_finite() { r[s] } else { 0.0 };

            // 1. TRUE PEAK DETECTION (4x inter-sample quadratic interpolation)
            let peak_l = in_l.abs();
            let peak_r = in_r.abs();

            // Interpolate the interval [z1, current] using the previous
            // sample as curvature information, and inspect four fractional
            // positions. This catches overs between discrete sample peaks.
            let mut isp_l = 0.0f32;
            let mut isp_r = 0.0f32;
            for step in 1..=4 {
                let t = step as f32 * 0.2;
                let w0 = 0.5 * t * (t - 1.0);
                let w1 = 1.0 - t * t;
                let w2 = 0.5 * t * (t + 1.0);
                isp_l = isp_l.max((w0 * self.z2_l + w1 * self.z1_l + w2 * in_l).abs());
                isp_r = isp_r.max((w0 * self.z2_r + w1 * self.z1_r + w2 * in_r).abs());
            }
            self.z2_l = self.z1_l;
            self.z2_r = self.z1_r;
            self.z1_l = in_l;
            self.z1_r = in_r;

            let max_peak = peak_l.max(peak_r).max(isp_l).max(isp_r);

            // 2. GAIN CALCULATION
            let mut target_gain = 1.0;
            if max_peak > threshold {
                target_gain = threshold / (max_peak + 1e-9);
            }

            // 3. ADAPTIVE RELEASE
            if target_gain < self.current_gain {
                self.current_gain = target_gain; // Instant Attack (Brickwall)
            } else {
                self.current_gain += (target_gain - self.current_gain) * 0.001; // Smooth Release
            }

            // 4. LOOK-AHEAD DELAY APPLICATION
            // Reusing DelayLineEngine which pushes and pops in one step.
            let delayed_l = self.delay_l.process(in_l, self.delay_samples as f32);
            let delayed_r = self.delay_r.process(in_r, self.delay_samples as f32);

            l[s] = delayed_l * self.current_gain * ceiling;
            r[s] = delayed_r * self.current_gain * ceiling;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide True Peak Limiter state.
    pub fn audit_true_peak_limiter(&self) -> bool {
        self.sample_rate.is_finite() && self.sample_rate >= 8_000.0
            && self.delay_samples > 0
            && self.delay_l.audit_delay_line() && self.delay_r.audit_delay_line()
            && self.current_gain.is_finite() && (0.0..=1.0).contains(&self.current_gain)
            && self.z2_l.is_finite() && self.z2_r.is_finite()
            && self.z1_l.is_finite() && self.z1_r.is_finite()
    }
}

#[cfg(test)]
mod tests {
    use super::TruePeakLimiterEngine;

    #[test]
    fn limiter_audit_catches_corrupted_state_and_processes_finite_audio() {
        let mut limiter = TruePeakLimiterEngine::new(48_000.0);
        assert!(limiter.audit_true_peak_limiter());
        let mut left = vec![2.0; 64];
        let mut right = vec![2.0; 64];
        limiter.process(&mut left, &mut right, -1.0, -1.0);
        assert!(left.iter().all(|sample| sample.is_finite()));
        limiter.current_gain = f32::NAN;
        assert!(!limiter.audit_true_peak_limiter());
    }
}
