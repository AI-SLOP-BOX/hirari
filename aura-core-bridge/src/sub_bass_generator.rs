pub struct SubBassGeneratorEngine {
    pub sample_rate: f64,
    pub env: f32,
    pub env_attack: f32,
    pub env_release: f32,
    pub lpf_state: f32,
    pub last_lpf: f32,
    pub dc_block_state: f32,
    pub lpf_coeff: f32,
    pub mix: f32,
    pub target_freq: f32,
    pub curr_freq: f32,
    pub phase: f64,
    pub zc_count: u32,
    pub is_positive: bool,
}

impl SubBassGeneratorEngine {
    pub fn new(sr: f64) -> Self {
        let mut engine = Self {
            sample_rate: sr,
            env: 0.0,
            env_attack: 0.0,
            env_release: 0.0,
            lpf_state: 0.0,
            last_lpf: 0.0,
            dc_block_state: 0.0,
            lpf_coeff: 0.0,
            mix: 0.5,
            target_freq: 50.0,
            curr_freq: 50.0,
            phase: 0.0,
            zc_count: 0,
            is_positive: false,
        };
        engine.update_coeffs();
        engine
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
        self.phase = 0.0;
        self.lpf_state = 0.0;
        self.dc_block_state = 0.0;
        self.zc_count = 0;
        self.target_freq = 50.0;
        self.curr_freq = 50.0;
        self.is_positive = false;
    }

    pub fn update_coeffs(&mut self) {
        let sr = self.sample_rate;
        self.env_attack = (-(1.0 / (0.005 * sr))).exp() as f32; // 5ms
        self.env_release = (-(1.0 / (0.1 * sr))).exp() as f32; // 100ms
        self.lpf_coeff = (-(1.0 / (0.001 * sr))).exp() as f32; // 1kHz LPF for tracking
    }

    pub fn set_mix(&mut self, mix: f32) {
        self.mix = mix;
    }

    /// INDUSTRIAL: Professional Sub-frequency Synthesis for Hip-Hop/EDM.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len();
        let sr = self.sample_rate as f32;

        for s in 0..len {
            let mid = (l[s] + r[s]) * 0.5;

            // 1. INPUT PRE-FILTER (LPF + DC Block for stable tracking)
            self.lpf_state += (1.0 - self.lpf_coeff) * (mid - self.lpf_state);
            self.dc_block_state = self.lpf_state - self.last_lpf + 0.995 * self.dc_block_state;
            self.last_lpf = self.lpf_state;

            // 2. ENVELOPE FOLLOWER
            let abs_in = mid.abs();
            if abs_in > self.env {
                self.env = self.env_attack * self.env + (1.0 - self.env_attack) * abs_in;
            } else {
                self.env *= self.env_release; // This is different from C++ which was m_env = m_envRelease * m_env;
                                              // Wait, C++ was: m_env = m_envRelease * m_env;
                                              // So self.env *= self.env_release is correct!
            }

            // 3. PITCH TRACKING (Zero-Crossing with Hysteresis)
            let mut triggered = false;
            if self.dc_block_state > 0.02 && !self.is_positive {
                self.is_positive = true;
                triggered = true;
            } else if self.dc_block_state < -0.02 && self.is_positive {
                self.is_positive = false;
            }

            if triggered {
                let period = self.zc_count as f32;
                if period > 10.0 {
                    let target = (sr / period) * 0.5;
                    self.target_freq = target.clamp(20.0, 90.0);
                }
                self.zc_count = 0;
            }
            self.zc_count = (self.zc_count + 1).min(10000);

            // 4. GENERATOR (Sub-Harmonic Sine)
            self.curr_freq += (self.target_freq - self.curr_freq) * 0.05;
            self.phase += self.curr_freq as f64 / self.sample_rate;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }

            let sub = (std::f64::consts::TAU * self.phase).sin() as f32 * self.env * self.mix;

            // 5. SUM
            l[s] += sub;
            r[s] += sub;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Sub Bass Generator state.
    pub fn audit_sub_bass_generator(&self) -> bool {
        self.sample_rate.is_finite() && self.sample_rate > 0.0
            && self.env.is_finite() && self.env >= 0.0
            && self.env_attack.is_finite() && (0.0..=1.0).contains(&self.env_attack)
            && self.env_release.is_finite() && (0.0..=1.0).contains(&self.env_release)
            && self.lpf_coeff.is_finite() && (0.0..=1.0).contains(&self.lpf_coeff)
            && self.mix.is_finite() && (0.0..=1.0).contains(&self.mix)
            && self.target_freq.is_finite() && (20.0..=90.0).contains(&self.target_freq)
            && self.curr_freq.is_finite() && self.curr_freq >= 0.0
            && self.phase.is_finite() && (0.0..1.0).contains(&self.phase)
    }
}
