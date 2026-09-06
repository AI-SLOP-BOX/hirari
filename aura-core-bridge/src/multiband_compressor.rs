pub struct BandComp {
    pub env: f32,
    pub gain: f32,
    pub sample_rate: f64,
}

impl BandComp {
    pub fn new(sr: f64) -> Self {
        Self {
            env: 0.0,
            gain: 1.0,
            sample_rate: sr,
        }
    }

    pub fn reset(&mut self) {
        self.gain = 1.0;
        self.env = 0.0;
    }

    pub fn process(&mut self, peak: f32) -> f32 {
        // Standard Compressor Logic: Threshold at -12dBFS
        let threshold = 0.25;
        let ratio = 4.0;

        let mut target_gain = 1.0;
        if peak > threshold {
            target_gain = (threshold / peak).powf(1.0 - 1.0 / ratio);
        }

        // Exponential Envelope Follower (Professional Grade)
        let attack = (-(1.0 / (0.010 * self.sample_rate))).exp() as f32; // 10ms
        let release = (-(1.0 / (0.100 * self.sample_rate))).exp() as f32; // 100ms

        let coeff = if target_gain < self.gain {
            attack
        } else {
            release
        };
        self.gain = coeff * self.gain + (1.0 - coeff) * target_gain;

        self.gain
    }
}

pub struct MultibandCompressorEngine {
    pub sample_rate: f64,
    pub low_mid_freq: f32,
    pub mid_high_freq: f32,
    pub low_band_unit: BandComp,
    pub mid_band_unit: BandComp,
    pub high_band_unit: BandComp,
    pub filters: [f32; 8],
}

impl MultibandCompressorEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            low_mid_freq: 200.0,
            mid_high_freq: 2500.0,
            low_band_unit: BandComp::new(sr),
            mid_band_unit: BandComp::new(sr),
            high_band_unit: BandComp::new(sr),
            filters: [0.0; 8],
        }
    }

    pub fn reset(&mut self) {
        self.low_band_unit.reset();
        self.mid_band_unit.reset();
        self.high_band_unit.reset();
        self.filters.fill(0.0);
    }

    pub fn set_split_freqs(&mut self, low_mid: f32, mid_high: f32) {
        let nyquist = (self.sample_rate as f32 * 0.49).max(20.0);
        if low_mid.is_finite()
            && mid_high.is_finite()
            && (20.0..nyquist).contains(&low_mid)
            && (low_mid..nyquist).contains(&mid_high)
        {
            self.low_mid_freq = low_mid;
            self.mid_high_freq = mid_high;
        }
    }

    fn process_lpf(&mut self, input: f32, freq: f32, idx: usize) -> f32 {
        let alpha = freq / (freq + self.sample_rate as f32);
        self.filters[idx] += alpha * (input - self.filters[idx]);
        self.filters[idx]
    }

    fn process_hpf(&mut self, input: f32, freq: f32, idx: usize) -> f32 {
        let alpha = freq / (freq + self.sample_rate as f32);
        self.filters[idx + 4] += alpha * (input - self.filters[idx + 4]);
        input - self.filters[idx + 4]
    }

    /// INDUSTRIAL: 3-Band Dynamics Processor with 1st-order complementary crossover.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len().min(r.len());
        if !self.audit_multiband_compressor() {
            return;
        }

        for s in 0..len {
            let in_l = if l[s].is_finite() { l[s] } else { 0.0 };
            let in_r = if r[s].is_finite() { r[s] } else { 0.0 };

            // 1. COMPLEMENTARY CROSSOVER (1st-order, 6dB/oct)
            let low_l = self.process_lpf(in_l, self.low_mid_freq, 0);
            let low_r = self.process_lpf(in_r, self.low_mid_freq, 1);

            let high_l = self.process_hpf(in_l, self.mid_high_freq, 2);
            let high_r = self.process_hpf(in_r, self.mid_high_freq, 3);

            let mid_l = in_l - low_l - high_l;
            let mid_r = in_r - low_r - high_r;

            // 2. INDEPENDENT BAND COMPRESSION (Exponential Envelope)
            let gain_l = self.low_band_unit.process(low_l.abs().max(low_r.abs()));
            let gain_m = self.mid_band_unit.process(mid_l.abs().max(mid_r.abs()));
            let gain_h = self.high_band_unit.process(high_l.abs().max(high_r.abs()));

            l[s] = (low_l * gain_l + mid_l * gain_m + high_l * gain_h).clamp(-1.0, 1.0);
            r[s] = (low_r * gain_l + mid_r * gain_m + high_r * gain_h).clamp(-1.0, 1.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Multiband Compressor state.
    pub fn audit_multiband_compressor(&self) -> bool {
        let nyquist = self.sample_rate * 0.5;
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.low_mid_freq.is_finite()
            && self.mid_high_freq.is_finite()
            && (20.0..nyquist as f32).contains(&self.low_mid_freq)
            && (self.low_mid_freq..nyquist as f32).contains(&self.mid_high_freq)
            && self
                .filters
                .iter()
                .all(|value| value.is_finite() && value.abs() <= 4.0)
            && [
                &self.low_band_unit,
                &self.mid_band_unit,
                &self.high_band_unit,
            ]
            .iter()
            .all(|band| {
                band.sample_rate.is_finite()
                    && (8_000.0..=384_000.0).contains(&band.sample_rate)
                    && band.env.is_finite()
                    && (0.0..=4.0).contains(&band.env)
                    && band.gain.is_finite()
                    && (0.0..=1.0).contains(&band.gain)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::MultibandCompressorEngine;

    #[test]
    fn multiband_processing_is_stereo_safe_and_finite() {
        let mut engine = MultibandCompressorEngine::new(48_000.0);
        engine.set_split_freqs(180.0, 2_400.0);
        let mut left = vec![0.8_f32; 512];
        let mut right = vec![0.4_f32; 256];
        engine.process(&mut left, &mut right);
        assert!(left[..256]
            .iter()
            .chain(right.iter())
            .all(|s| s.is_finite() && s.abs() <= 1.0));
        assert!(engine.audit_multiband_compressor());
    }

    #[test]
    fn multiband_audit_rejects_corrupt_state_and_split_updates() {
        let mut engine = MultibandCompressorEngine::new(48_000.0);
        engine.set_split_freqs(20_000.0, 100.0);
        assert_eq!(engine.low_mid_freq, 200.0);
        engine.filters[0] = f32::NAN;
        assert!(!engine.audit_multiband_compressor());
    }
}
