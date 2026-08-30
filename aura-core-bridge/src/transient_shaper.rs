pub struct TransientShaperEngine {
    pub sample_rate: f64,
    pub attack: f32,
    pub sustain: f32,
    pub attack_env: f32,
    pub sustain_env: f32,
    pub attack_alpha: f32,
    pub sustain_alpha: f32,
    pub current_gain: f32,
}

impl TransientShaperEngine {
    pub fn new(sr: f64) -> Self {
        let sr = if sr.is_finite() {
            sr.clamp(8_000.0, 384_000.0)
        } else {
            48_000.0
        };
        let mut engine = Self {
            sample_rate: sr,
            attack: 0.0,
            sustain: 0.0,
            attack_env: -60.0,
            sustain_env: -60.0,
            attack_alpha: 0.9,
            sustain_alpha: 0.99,
            current_gain: 1.0,
        };
        engine.update_ballistics();
        engine
    }

    pub fn reset(&mut self) {
        self.attack_env = -60.0;
        self.sustain_env = -60.0;
        self.current_gain = 1.0;
    }

    pub fn update_ballistics(&mut self) {
        if !self.sample_rate.is_finite() || self.sample_rate <= 0.0 {
            self.sample_rate = 48_000.0;
        }
        // Attack envelope: Fast (approx 4ms)
        self.attack_alpha = (-(1.0 / (self.sample_rate * 0.004))).exp() as f32;
        // Sustain envelope: Slow (approx 60ms)
        self.sustain_alpha = (-(1.0 / (self.sample_rate * 0.060))).exp() as f32;
    }

    pub fn set_attack(&mut self, a: f32) {
        self.attack = a;
    }

    pub fn set_sustain(&mut self, s: f32) {
        self.sustain = s;
    }

    /**
     * @brief PROCESS: Applies transient shaping in the Logarithmic/Decibel domain.
     * INDUSTRIAL:
     *  - Replaces simplified ratio math with dB domain difference calculations (SPL-style).
     *  - Automatically gates sustain boosting below -60 dB to prevent amplifying vocal breath or quiet noise floors.
     *  - Clamps output gain to a safe +/-24 dB range to prevent signal blowout.
     */
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len().min(r.len());

        for s in 0..len {
            let in_l = if l[s].is_finite() { l[s] } else { 0.0 };
            let in_r = if r[s].is_finite() { r[s] } else { 0.0 };
            let level = in_l.abs().max(in_r.abs());
            let level_db = 20.0f32 * (level + 1e-5f32).log10();

            // 1. Dual Envelope Followers in the logarithmic domain
            self.attack_env =
                self.attack_alpha * self.attack_env + (1.0 - self.attack_alpha) * level_db;
            self.sustain_env =
                self.sustain_alpha * self.sustain_env + (1.0 - self.sustain_alpha) * level_db;

            // 2. Transients are the direct difference between fast and slow envelopes
            let diff_db = self.attack_env - self.sustain_env;

            // 3. Attack shaping (amplifies or attenuates onsets)
            let attack_gain_db = diff_db * self.attack;

            // 4. Sustain shaping (extends or dampens decay)
            // Prevent noise floor amplification by gating sustain modification when the input is too quiet
            let sustain_gain_db = if level_db > -60.0 {
                (self.sustain_env - level_db) * self.sustain
            } else {
                0.0
            };

            // 5. Recombine, clamp to a safe +/-24 dB range, and convert to linear gain
            let total_gain_db = (attack_gain_db + sustain_gain_db).clamp(-24.0, 24.0);
            let target_gain = 10.0f32.powf(total_gain_db / 20.0);

            // 6. Smooth gain interpolation to suppress zipper noise
            self.current_gain = 0.9 * self.current_gain + 0.1 * target_gain;

            l[s] *= self.current_gain;
            r[s] *= self.current_gain;
        }
    }

    pub fn audit_transient_shaper(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.attack.is_finite()
            && self.sustain.is_finite()
            && self.attack_env.is_finite()
            && self.sustain_env.is_finite()
            && self.attack_alpha.is_finite()
            && (0.0..1.0).contains(&self.attack_alpha)
            && self.sustain_alpha.is_finite()
            && (0.0..1.0).contains(&self.sustain_alpha)
            && self.current_gain.is_finite()
            && self.current_gain >= 0.0
    }
}
