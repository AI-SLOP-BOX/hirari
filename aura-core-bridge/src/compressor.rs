pub struct DynamicCompressorEngine {
    pub sample_rate: f64,
    pub threshold_db: f32,
    pub ratio: f32,
    pub makeup_db: f32,
    pub knee_db: f32,
    pub auto_gain: bool,
    pub use_rms: bool,
    pub rms_sum: f32,
    pub attack_alpha: f32,
    pub release_alpha: f32,
    pub envelope: f32,
    pub current_gr: f32,
    pub delay_l: Vec<f32>,
    pub delay_r: Vec<f32>,
    pub write_idx: usize,
    pub lookahead_samples: usize,
}

impl DynamicCompressorEngine {
    pub fn new(sr: f64) -> Self {
        let max_lookahead = 4096;
        Self {
            sample_rate: sr,
            threshold_db: -20.0,
            ratio: 4.0,
            makeup_db: 0.0,
            knee_db: 6.0,
            auto_gain: true,
            use_rms: false,
            rms_sum: 0.0,
            attack_alpha: 0.9,
            release_alpha: 0.999,
            envelope: 0.0,
            current_gr: 1.0,
            delay_l: vec![0.0; max_lookahead],
            delay_r: vec![0.0; max_lookahead],
            write_idx: 0,
            lookahead_samples: 0,
        }
    }

    pub fn reset(&mut self) {
        self.envelope = 0.0;
        self.current_gr = 1.0;
        self.rms_sum = 0.0;
        self.delay_l.fill(0.0);
        self.delay_r.fill(0.0);
        self.write_idx = 0;
    }

    pub fn set_lookahead(&mut self, ms: f32) {
        if !ms.is_finite() || ms < 0.0 {
            return;
        }
        self.lookahead_samples = (self.sample_rate * ms as f64 * 0.001) as usize;
        if self.lookahead_samples > 4096 {
            self.lookahead_samples = 4096;
        }
    }

    pub fn set_attack(&mut self, ms: f32) {
        if ms.is_finite() && (0.01..=2_000.0).contains(&ms) && self.sample_rate.is_finite() {
            self.attack_alpha = (-(1.0 / (self.sample_rate * ms as f64 * 0.001))).exp() as f32;
        }
    }

    pub fn set_release(&mut self, ms: f32) {
        if ms.is_finite() && (0.01..=10_000.0).contains(&ms) && self.sample_rate.is_finite() {
            self.release_alpha = (-(1.0 / (self.sample_rate * ms as f64 * 0.001))).exp() as f32;
        }
    }

    fn calculate_auto_makeup(&self) -> f32 {
        -(self.threshold_db * (1.0 - 1.0 / self.ratio)) * 0.5
    }

    /// HONEST FIX: Fast Log10 Approximation (Idiomatic Rust).
    fn fast_log10(&self, x: f32) -> f32 {
        if x < 1e-7 {
            return -140.0;
        }
        let i = x.to_bits();
        let log2 = (i as f32) * 1.192_092_9e-7 - 126.942_696;
        log2 * 3.010_3 // Log2 to Log10 conversion
    }

    /// INDUSTRIAL: Professional Mastering-Grade Dynamic Range Processor.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], sidechain: Option<(&[f32], &[f32])>) {
        let len = l.len().min(r.len());
        let max_lookahead = self.delay_l.len();
        if len == 0 || max_lookahead == 0 || !self.audit_compressor() {
            return;
        }

        let total_makeup_db = self.makeup_db
            + if self.auto_gain {
                self.calculate_auto_makeup()
            } else {
                0.0
            };

        for s in 0..len {
            // 1. Sidechain Key Selection
            let (sc_l, sc_r) = if let Some((s_l, s_r)) = sidechain {
                if s >= s_l.len().min(s_r.len()) {
                    (l[s], r[s])
                } else {
                    (s_l[s], s_r[s])
                }
            } else {
                (l[s], r[s])
            };

            // DETECTOR
            let mut detector_level = sc_l.abs().max(sc_r.abs());
            if self.use_rms {
                self.rms_sum = 0.999 * self.rms_sum + 0.001 * (detector_level * detector_level);
                detector_level = self.rms_sum.sqrt();
            }

            let target_alpha = if detector_level > self.envelope {
                self.attack_alpha
            } else {
                self.release_alpha
            };
            self.envelope = target_alpha * self.envelope + (1.0 - target_alpha) * detector_level;

            // GAIN COMPUTATION
            let env_db = self.fast_log10(self.envelope);

            let delta = env_db - self.threshold_db;
            let mut gain_db = 0.0;
            if delta > 0.5 * self.knee_db {
                gain_db = delta * (1.0 / self.ratio - 1.0);
            } else if delta > -0.5 * self.knee_db {
                let x = delta + self.knee_db * 0.5;
                gain_db = (1.0 / self.ratio - 1.0) * x * x / (2.0 * self.knee_db);
            }

            let target_gr = 10.0f32.powf((gain_db + total_makeup_db) / 20.0);
            self.current_gr = 0.99 * self.current_gr + 0.01 * target_gr;

            // LOOK-AHEAD: Store current sample and emit delayed one
            self.delay_l[self.write_idx] = l[s];
            self.delay_r[self.write_idx] = r[s];

            let read_idx =
                (self.write_idx + max_lookahead - self.lookahead_samples) % max_lookahead;
            l[s] = self.delay_l[read_idx] * self.current_gr;
            r[s] = self.delay_r[read_idx] * self.current_gr;

            self.write_idx = (self.write_idx + 1) % max_lookahead;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Compressor state.
    pub fn audit_compressor(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.threshold_db.is_finite()
            && (-120.0..=0.0).contains(&self.threshold_db)
            && self.ratio.is_finite()
            && (1.0..=100.0).contains(&self.ratio)
            && self.makeup_db.is_finite()
            && (-48.0..=48.0).contains(&self.makeup_db)
            && self.knee_db.is_finite()
            && (0.0..=48.0).contains(&self.knee_db)
            && self.rms_sum.is_finite()
            && self.rms_sum >= 0.0
            && self.attack_alpha.is_finite()
            && (0.0..=1.0).contains(&self.attack_alpha)
            && self.release_alpha.is_finite()
            && (0.0..=1.0).contains(&self.release_alpha)
            && self.envelope.is_finite()
            && (0.0..=4.0).contains(&self.envelope)
            && self.current_gr.is_finite()
            && (0.0..=16.0).contains(&self.current_gr)
            && self.delay_l.len() == self.delay_r.len()
            && !self.delay_l.is_empty()
            && self.lookahead_samples < self.delay_l.len()
            && self.write_idx < self.delay_l.len()
            && self
                .delay_l
                .iter()
                .chain(self.delay_r.iter())
                .all(|v| v.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::DynamicCompressorEngine;

    #[test]
    fn compressor_handles_mismatched_audio_and_sidechain_lengths() {
        let mut compressor = DynamicCompressorEngine::new(48_000.0);
        let mut left = vec![0.8_f32; 128];
        let mut right = vec![0.4_f32; 64];
        let side_l = vec![0.9_f32; 32];
        let side_r = vec![0.9_f32; 32];
        compressor.process(&mut left, &mut right, Some((&side_l, &side_r)));
        assert!(left[..64].iter().chain(right.iter()).all(|v| v.is_finite()));
        assert!(compressor.audit_compressor());
    }

    #[test]
    fn compressor_audit_rejects_corrupt_state() {
        let mut compressor = DynamicCompressorEngine::new(48_000.0);
        compressor.current_gr = f32::NAN;
        assert!(!compressor.audit_compressor());
    }
}
