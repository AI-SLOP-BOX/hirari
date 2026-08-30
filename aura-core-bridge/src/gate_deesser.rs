const MAX_LOOKAHEAD_SAMPLES: usize = 128;

pub struct DeEsserEngine {
    pub sample_rate: f64,
    pub bb_env: f32,
    pub sib_env: f32,
    pub gain: f32,
    attack_coeff: f32,
    release_coeff: f32,
    hp_coeff: f32,
    lp_coeff: f32,
    hp_state: [f32; 2],
    lp_state: [f32; 2],
    delay_l: [f32; MAX_LOOKAHEAD_SAMPLES],
    delay_r: [f32; MAX_LOOKAHEAD_SAMPLES],
    delay_index: usize,
    lookahead_samples: usize,
}

impl DeEsserEngine {
    pub fn new(sample_rate: f64) -> Self {
        let mut engine = Self {
            sample_rate,
            bb_env: 0.0,
            sib_env: 0.0,
            gain: 1.0,
            attack_coeff: 0.0,
            release_coeff: 0.0,
            hp_coeff: 0.0,
            lp_coeff: 0.0,
            hp_state: [0.0; 2],
            lp_state: [0.0; 2],
            delay_l: [0.0; MAX_LOOKAHEAD_SAMPLES],
            delay_r: [0.0; MAX_LOOKAHEAD_SAMPLES],
            delay_index: 0,
            lookahead_samples: 1,
        };
        engine.update_coefficients();
        engine
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
        self.update_coefficients();
        self.reset_delay();
    }

    fn update_coefficients(&mut self) {
        let sr = if self.sample_rate.is_finite() && self.sample_rate > 0.0 {
            self.sample_rate
        } else {
            44100.0
        };
        self.lookahead_samples = ((sr * 0.001).round() as usize).clamp(1, MAX_LOOKAHEAD_SAMPLES);
        self.attack_coeff = (1.0 - (-1.0 / (0.002 * sr)).exp()) as f32;
        self.release_coeff = (1.0 - (-1.0 / (0.050 * sr)).exp()) as f32;
        // Keep the detector band below Nyquist.  Without this clamp a low-rate
        // device turns the nominal low-pass into an invalid detector.
        let nyquist = (sr * 0.5).max(1.0);
        let hp_cutoff = (3500.0_f64).min(nyquist * 0.45).max(1.0);
        let lp_cutoff = (12000.0_f64).min(nyquist * 0.90).max(hp_cutoff);
        self.hp_coeff = (1.0 - (-std::f64::consts::TAU * hp_cutoff / sr).exp()) as f32;
        self.lp_coeff = (1.0 - (-std::f64::consts::TAU * lp_cutoff / sr).exp()) as f32;
    }

    fn reset_delay(&mut self) {
        self.delay_l = [0.0; MAX_LOOKAHEAD_SAMPLES];
        self.delay_r = [0.0; MAX_LOOKAHEAD_SAMPLES];
        self.delay_index = 0;
    }

    /// Processes a block using a small, allocation-free high-frequency detector.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let num_samples = left.len().min(right.len());
        let attack = self.attack_coeff;
        let release = self.release_coeff;
        let hp = self.hp_coeff;
        let lp = self.lp_coeff;

        let detect = |x: f32, hp_state: &mut f32, lp_state: &mut f32| {
            let x = if x.is_finite() {
                x.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            // Cascaded one-pole filters form a stable, inexpensive 3.5--12 kHz band.
            *hp_state += hp * (x - *hp_state);
            let high = x - *hp_state;
            *lp_state += lp * (high - *lp_state);
            *lp_state
        };

        for i in 0..num_samples {
            let in_l = if left[i].is_finite() {
                left[i].clamp(-1.0, 1.0)
            } else {
                0.0
            };
            let in_r = if right[i].is_finite() {
                right[i].clamp(-1.0, 1.0)
            } else {
                0.0
            };
            // Read the sample that entered the fixed delay line first. The
            // current input is then written into that slot, so the detector
            // below can drive gain for audio that is one lookahead window
            // behind it. This keeps the RT path allocation-free.
            let delayed_l = self.delay_l[self.delay_index];
            let delayed_r = self.delay_r[self.delay_index];
            self.delay_l[self.delay_index] = in_l;
            self.delay_r[self.delay_index] = in_r;
            let broadband_env = (in_l.abs() + in_r.abs()) * 0.5;
            self.bb_env += (broadband_env - self.bb_env) * 0.01;
            let sibilance_env = (detect(in_l, &mut self.hp_state[0], &mut self.lp_state[0]).abs()
                + detect(in_r, &mut self.hp_state[1], &mut self.lp_state[1]).abs())
                * 0.5;
            if sibilance_env > self.sib_env {
                self.sib_env += (sibilance_env - self.sib_env) * attack;
            } else {
                self.sib_env += (sibilance_env - self.sib_env) * release;
            }

            // 2. ADAPTIVE COMPRESSION
            let sib_ratio = self.sib_env / self.bb_env.max(1e-6);
            let mut target_gain = 1.0;
            let threshold = 2.0;

            if sib_ratio > threshold {
                let intensity = (sib_ratio - threshold) * 2.5;
                let intensity = intensity.min(1.0);
                target_gain = 1.0 - (intensity * 0.4); // Max 6dB reduction
            }

            self.gain += (target_gain - self.gain)
                * (if target_gain < self.gain {
                    attack
                } else {
                    release
                });
            if !self.gain.is_finite() {
                self.gain = 1.0;
            }

            // Apply the gain driven by the current detector to delayed audio.
            // The detector therefore sees the upcoming signal before the
            // corresponding sample reaches the output.
            left[i] = (delayed_l * self.gain).clamp(-1.0, 1.0);
            right[i] = (delayed_r * self.gain).clamp(-1.0, 1.0);
            self.delay_index = (self.delay_index + 1) % self.lookahead_samples;
        }
    }
}

pub struct NoiseGateEngine {
    pub sample_rate: f64,
    pub env: f32,
    pub gain: f32,
    pub hold_counter: u32,
    pub is_opening: bool,
}

impl NoiseGateEngine {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            env: 0.0,
            gain: 0.0,
            hold_counter: 0,
            is_opening: false,
        }
    }

    /// INDUSTRIAL: Processes an audio block with sidechain support.
    pub fn process_with_sidechain(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        sc_left: &[f32],
        sc_right: &[f32],
    ) {
        let num_samples = left.len().min(right.len());
        let safe_sr = if self.sample_rate.is_finite() && self.sample_rate > 0.0 {
            self.sample_rate
        } else {
            44100.0
        };
        let attack = 1.0 - (-1.0 / (0.002 * safe_sr as f32)).exp();
        let release = 1.0 - (-1.0 / (0.2 * safe_sr as f32)).exp();

        for i in 0..num_samples {
            let sc_l_val = sc_left.get(i).copied().unwrap_or(left[i]);
            let sc_r_val = sc_right.get(i).copied().unwrap_or(right[i]);
            let sc_l_val = if sc_l_val.is_finite() { sc_l_val } else { 0.0 };
            let sc_r_val = if sc_r_val.is_finite() { sc_r_val } else { 0.0 };
            let inst_env = (sc_l_val.abs() + sc_r_val.abs()) * 0.5;

            // Peak Detector with asymmetric time constants
            if inst_env > self.env {
                self.env += (inst_env - self.env) * 0.01;
            } else {
                self.env += (inst_env - self.env) * 0.0005;
            }

            if self.env > 0.01 {
                self.is_opening = true;
                self.hold_counter = (safe_sr * 0.05) as u32; // 50ms Hold
            } else if self.env < 0.005 {
                if self.hold_counter > 0 {
                    self.hold_counter -= 1;
                } else {
                    self.is_opening = false;
                }
            }

            self.gain += ((if self.is_opening { 1.0 } else { 0.0 }) - self.gain)
                * (if self.is_opening { attack } else { release });
            if !self.env.is_finite() {
                self.env = 0.0;
            }
            if !self.gain.is_finite() {
                self.gain = 0.0;
            }

            // Apply the smoothed gate gain to the current sample.
            left[i] =
                ((if left[i].is_finite() { left[i] } else { 0.0 }) * self.gain).clamp(-1.0, 1.0);
            right[i] =
                ((if right[i].is_finite() { right[i] } else { 0.0 }) * self.gain).clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DeEsserEngine;

    #[test]
    fn lookahead_delays_audio_without_allocating() {
        let mut engine = DeEsserEngine::new(48_000.0);
        let mut left = vec![0.0; 96];
        let mut right = vec![0.0; 96];
        left[0] = 0.5;
        right[0] = 0.5;

        engine.process(&mut left, &mut right);

        assert_eq!(left[0], 0.0);
        assert_eq!(right[0], 0.0);
        assert!(left[48].is_finite());
        assert!(right[48].is_finite());
        assert!(left.iter().any(|sample| *sample != 0.0));
    }

    #[test]
    fn handles_empty_short_and_non_finite_buffers() {
        let mut engine = DeEsserEngine::new(0.0);
        let mut empty_left: [f32; 0] = [];
        let mut empty_right: [f32; 0] = [];
        engine.process(&mut empty_left, &mut empty_right);

        let mut left = [f32::NAN, f32::INFINITY, -f32::INFINITY];
        let mut right = [f32::NAN, 1.0, -1.0];
        engine.process(&mut left, &mut right);

        assert!(left.iter().all(|sample| sample.is_finite()));
        assert!(right.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn low_sample_rate_keeps_delay_and_output_stable() {
        let mut engine = DeEsserEngine::new(8_000.0);
        let mut left = [0.25; 16];
        let mut right = [0.25; 16];

        engine.process(&mut left, &mut right);

        assert!(left.iter().all(|sample| sample.is_finite()));
        assert!(right.iter().all(|sample| sample.is_finite()));
    }
}
