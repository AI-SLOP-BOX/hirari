pub struct TransientInfo {
    pub sample_index: u64,
    pub strength: f32,
}

pub struct TransientOrchestrator {
    pub sample_rate: f64,
    pub env_fast: f32,
    pub env_slow: f32,
    pub alpha_fast: f32,
    pub alpha_slow: f32,
    pub last_transient_idx: Option<usize>,
}

impl TransientOrchestrator {
    pub fn new(sr: f64) -> Self {
        let sr = if sr.is_finite() {
            sr.clamp(8_000.0, 384_000.0)
        } else {
            48_000.0
        };
        let alpha_fast = (-1.0 / (sr * 0.005)) as f32; // 5ms
        let alpha_slow = (-1.0 / (sr * 0.050)) as f32; // 50ms

        Self {
            sample_rate: sr,
            env_fast: 0.0,
            env_slow: 0.0,
            alpha_fast: alpha_fast.exp(),
            alpha_slow: alpha_slow.exp(),
            last_transient_idx: None,
        }
    }

    /// INDUSTRIAL: Performs transient analysis with absolute precision and transient sovereignty.
    pub fn analyze_transients(&mut self, buffer: &[f32], threshold: f32) -> Vec<TransientInfo> {
        let len = buffer.len();
        let mut transients = Vec::new();
        let min_interval = (self.sample_rate * 0.020) as usize; // 20ms lockout
        let threshold = if threshold.is_finite() {
            threshold.max(0.0)
        } else {
            0.0
        };

        for i in 0..len {
            let val = if buffer[i].is_finite() {
                buffer[i].abs()
            } else {
                0.0
            };

            // Update envelopes
            self.env_fast = self.alpha_fast * self.env_fast + (1.0 - self.alpha_fast) * val;
            self.env_slow = self.alpha_slow * self.env_slow + (1.0 - self.alpha_slow) * val;

            // Transient detection (Energy jump)
            if self.env_fast > self.env_slow * (1.0 + threshold) {
                let should_trigger = match self.last_transient_idx {
                    Some(last_idx) => i - last_idx > min_interval,
                    None => true,
                };

                if should_trigger {
                    transients.push(TransientInfo {
                        sample_index: i as u64,
                        strength: self.env_fast / self.env_slow,
                    });
                    self.last_transient_idx = Some(i);
                }
            }
        }

        transients
    }

    pub fn audit_transients(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.env_fast.is_finite()
            && self.env_slow.is_finite()
            && self.alpha_fast.is_finite()
            && (0.0..1.0).contains(&self.alpha_fast)
            && self.alpha_slow.is_finite()
            && (0.0..1.0).contains(&self.alpha_slow)
    }
}
