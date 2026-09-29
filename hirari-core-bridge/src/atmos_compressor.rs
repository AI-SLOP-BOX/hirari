pub struct AtmosCompressorEngine {
    pub sample_rate: f64,
    pub env: f32,
}

impl AtmosCompressorEngine {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate: if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate)
            {
                sample_rate
            } else {
                48_000.0
            },
            env: 0.0,
        }
    }

    /// INDUSTRIAL: Processes an immersive block with linked compression.
    pub fn process_immersive(
        &mut self,
        buffers: &mut [&mut [f32]],
        threshold: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
    ) {
        if buffers.is_empty()
            || !self.audit_atmos_compressor()
            || !threshold.is_finite()
            || !(-120.0..=6.0).contains(&threshold)
            || !ratio.is_finite()
            || !(1.0..=100.0).contains(&ratio)
            || !attack_ms.is_finite()
            || !(0.01..=10_000.0).contains(&attack_ms)
            || !release_ms.is_finite()
            || !(0.01..=30_000.0).contains(&release_ms)
        {
            return;
        }
        let num_samples = buffers.iter().map(|buffer| buffer.len()).min().unwrap_or(0);

        let attack = 1.0 - (-1.0 / (attack_ms * 0.001 * self.sample_rate as f32)).exp();
        let release = 1.0 - (-1.0 / (release_ms * 0.001 * self.sample_rate as f32)).exp();

        for i in 0..num_samples {
            // 1. SIDECHAIN SUM (Max peak across all Atmos channels)
            let mut max_peak = 0.0f32;
            for buf in buffers.iter() {
                let sample = if buf[i].is_finite() { buf[i] } else { 0.0 };
                max_peak = max_peak.max(sample.abs());
            }

            // 2. ENVELOPE FOLLOWER
            if max_peak > self.env {
                self.env += (max_peak - self.env) * attack;
            } else {
                self.env += (max_peak - self.env) * release;
            }

            // 3. GAIN CALCULATION
            let mut reduction = 1.0f32;
            let env_db = 20.0 * (self.env + 1e-10).log10();
            if env_db > threshold {
                let over_db = env_db - threshold;
                let target_db = threshold + over_db / ratio;
                reduction = (10.0f32).powf((target_db - env_db) / 20.0);
            }

            // 4. APPLY TO ALL CHANNELS
            for buf in buffers.iter_mut() {
                buf[i] = (if buf[i].is_finite() {
                    buf[i] * reduction
                } else {
                    0.0
                })
                .clamp(-4.0, 4.0);
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Atmos state.
    pub fn audit_atmos_compressor(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.env.is_finite()
            && (0.0..=4.0).contains(&self.env)
    }
}
