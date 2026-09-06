pub struct SidechainCompressorEngine {
    pub sample_rate: f64,
    pub threshold: f32,
    pub ratio: f32,
    pub attack: f32,
    pub release: f32,
    pub env: f32,
    pub current_gain: f32,
}

impl SidechainCompressorEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            threshold: 0.2,
            ratio: 10.0,
            attack: 10.0,
            release: 100.0,
            env: 0.0,
            current_gain: 1.0,
        }
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
        self.current_gain = 1.0;
    }

    pub fn set_params(&mut self, threshold: f32, ratio: f32, attack: f32, release: f32) {
        self.threshold = threshold;
        self.ratio = ratio;
        self.attack = attack;
        self.release = release;
    }

    pub fn try_set_params(
        &mut self,
        threshold: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
    ) -> bool {
        if !threshold.is_finite()
            || !(1e-5..=4.0).contains(&threshold)
            || !ratio.is_finite()
            || !(1.0..=1000.0).contains(&ratio)
            || !attack_ms.is_finite()
            || !(0.01..=10_000.0).contains(&attack_ms)
            || !release_ms.is_finite()
            || !(0.01..=30_000.0).contains(&release_ms)
        {
            return false;
        }
        self.set_params(threshold, ratio, attack_ms, release_ms);
        true
    }

    /// INDUSTRIAL: Professional High-performance Ducking Engine using the Sidechain input.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], sidechain: Option<(&[f32], &[f32])>) {
        let len = l.len().min(r.len());
        if len == 0 || !self.audit_sidechain_compressor() {
            return;
        }

        let alpha_a = (-(1.0 / (self.sample_rate * self.attack as f64 * 0.001))).exp() as f32;
        let alpha_r = (-(1.0 / (self.sample_rate * self.release as f64 * 0.001))).exp() as f32;

        for s in 0..len {
            // 1. SIDECHAIN DETECTOR
            let (sc_l, sc_r) = if let Some((s_l, s_r)) = sidechain {
                (
                    s_l.get(s).copied().unwrap_or(0.0),
                    s_r.get(s).copied().unwrap_or(0.0),
                )
            } else {
                (l[s], r[s])
            };

            let in_level = (if sc_l.is_finite() { sc_l.abs() } else { 0.0 })
                .max(if sc_r.is_finite() { sc_r.abs() } else { 0.0 });

            // Peak Detection
            let alpha = if in_level > self.env {
                alpha_a
            } else {
                alpha_r
            };
            self.env = alpha * self.env + (1.0 - alpha) * in_level;

            // Gain Reduction Logic
            let mut reduction = 1.0;
            if self.env > self.threshold {
                let db_over = 20.0 * (self.env / self.threshold).log10();
                let db_reduced = db_over * (1.0 / self.ratio - 1.0);
                reduction = 10.0f32.powf(db_reduced / 20.0);
            }

            // Smoothing for gain (Avoid Zipper noise)
            self.current_gain = 0.95 * self.current_gain + 0.05 * reduction;

            l[s] =
                ((if l[s].is_finite() { l[s] } else { 0.0 }) * self.current_gain).clamp(-4.0, 4.0);
            r[s] =
                ((if r[s].is_finite() { r[s] } else { 0.0 }) * self.current_gain).clamp(-4.0, 4.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Sidechain Compressor state.
    pub fn audit_sidechain_compressor(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.threshold.is_finite()
            && (1e-5..=4.0).contains(&self.threshold)
            && self.ratio.is_finite()
            && (1.0..=1000.0).contains(&self.ratio)
            && self.attack.is_finite()
            && (0.01..=10_000.0).contains(&self.attack)
            && self.release.is_finite()
            && (0.01..=30_000.0).contains(&self.release)
            && self.env.is_finite()
            && (0.0..=4.0).contains(&self.env)
            && self.current_gain.is_finite()
            && (0.0..=1.0).contains(&self.current_gain)
    }
}

#[cfg(test)]
mod tests {
    use super::SidechainCompressorEngine;

    #[test]
    fn external_sidechain_reduces_program_audio() {
        let mut compressor = SidechainCompressorEngine::new(48_000.0);
        compressor.set_params(0.1, 10.0, 0.1, 20.0);
        let mut left = vec![0.8; 512];
        let mut right = vec![0.8; 512];
        let side_left = vec![1.0; 512];
        let side_right = vec![1.0; 512];

        compressor.process(&mut left, &mut right, Some((&side_left, &side_right)));

        assert!(compressor.current_gain < 1.0);
        assert!(left
            .iter()
            .all(|sample| sample.is_finite() && *sample < 0.8));
        assert!(right
            .iter()
            .all(|sample| sample.is_finite() && *sample < 0.8));
        assert!(compressor.audit_sidechain_compressor());
    }

    #[test]
    fn missing_sidechain_falls_back_to_program_detector() {
        let mut compressor = SidechainCompressorEngine::new(48_000.0);
        compressor.set_params(0.1, 4.0, 1.0, 20.0);
        let mut left = vec![0.7; 256];
        let mut right = vec![0.7; 256];
        compressor.process(&mut left, &mut right, None);

        assert!(compressor.current_gain < 1.0);
        assert!(left.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn validated_parameter_updates_preserve_previous_state_on_error() {
        let mut compressor = SidechainCompressorEngine::new(48_000.0);
        assert!(compressor.try_set_params(0.2, 4.0, 5.0, 100.0));
        assert!(!compressor.try_set_params(0.0, 4.0, 5.0, 100.0));
        assert_eq!((compressor.threshold, compressor.ratio), (0.2, 4.0));
        assert!(compressor.audit_sidechain_compressor());
    }
}
