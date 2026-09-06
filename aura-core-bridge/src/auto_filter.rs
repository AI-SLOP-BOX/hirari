pub struct AutoFilterEngine {
    pub sample_rate: f64,
    pub cutoff_base: f32,
    pub res: f32,
    pub sens: f32,
    pub env: f32,
    pub attack: f32,
    pub release: f32,
    pub g: f32,
    pub k: f32,
    pub s1: [f32; 2],
    pub s2: [f32; 2],
}

impl AutoFilterEngine {
    pub fn new(sr: f64) -> Self {
        let sr = if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) {
            sr
        } else {
            48_000.0
        };
        let mut engine = Self {
            sample_rate: sr,
            cutoff_base: 0.2,
            res: 0.3,
            sens: 0.8,
            env: 0.0,
            attack: 0.0,
            release: 0.0,
            g: 0.0,
            k: 0.0,
            s1: [0.0; 2],
            s2: [0.0; 2],
        };
        engine.update_time_constants();
        engine
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
        self.s1 = [0.0; 2];
        self.s2 = [0.0; 2];
    }

    pub fn set_parameters(&mut self, cutoff: f32, res: f32, sens: f32) {
        self.cutoff_base = cutoff.clamp(0.0, 1.0);
        self.res = res.clamp(0.0, 1.0);
        self.sens = sens.clamp(0.0, 1.0);
    }

    fn update_time_constants(&mut self) {
        self.attack = 1.0 - (-1.0 / (0.005 * self.sample_rate)).exp() as f32; // 5ms
        self.release = 1.0 - (-1.0 / (0.100 * self.sample_rate)).exp() as f32; // 100ms
    }

    /// INDUSTRIAL: Professional Dynamic Resonant Filter (Auto-Wah).
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        // A non-positive or non-finite sample rate cannot produce meaningful
        // filter coefficients.  Leave the input untouched rather than
        // allowing NaNs/Infs to enter the state variables.
        if !self.audit_auto_filter() {
            return;
        }

        let len = l.len().min(r.len());

        for s in 0..len {
            let in_l = l[s];
            let in_r = r[s];
            let mid = (in_l + in_r) * 0.5;

            if !self.env.is_finite() {
                self.env = 0.0;
            }
            let attack = if self.attack.is_finite() {
                self.attack.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let release = if self.release.is_finite() {
                self.release.clamp(0.0, 1.0)
            } else {
                0.0
            };

            // 1. Envelope Follower (Correct timing)
            let abs_in = mid.abs();
            if abs_in > self.env {
                self.env += (abs_in - self.env) * attack;
            } else {
                self.env += (abs_in - self.env) * release;
            }

            // 2. Sample-Accurate Modulated Cutoff
            let sens = if self.sens.is_finite() {
                self.sens.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let cutoff_base = if self.cutoff_base.is_finite() {
                self.cutoff_base.clamp(0.0, 1.0)
            } else {
                0.2
            };
            let depth = self.env * sens;
            let target_cutoff = (cutoff_base + depth).clamp(0.02, 0.98);

            // Continuous g-coefficient smoothing (No more clicking)
            let nyquist = self.sample_rate * 0.5;
            let requested_f = target_cutoff as f64 * 8000.0;
            // Keep tan() strictly below pi/2.  The margin is immaterial for
            // ordinary rates/cutoffs, but prevents an infinite g at Nyquist.
            let f = requested_f.min(nyquist * (1.0 - 1.0e-6));
            let target_g = (std::f64::consts::PI * f / self.sample_rate).tan();
            let target_g = if target_g.is_finite() {
                target_g.max(0.0) as f32
            } else {
                0.0
            };
            let current_g = if self.g.is_finite() {
                self.g.max(0.0)
            } else {
                0.0
            };
            self.g = current_g + (target_g - current_g) * 0.2; // Smooth ramp
            let res = if self.res.is_finite() {
                self.res.clamp(0.0, 1.0)
            } else {
                0.3
            };
            self.k = 2.0 - (res * 1.95);

            // 3. SVF Execution (Stereo)
            for c in 0..2 {
                let in_val = if c == 0 { in_l } else { in_r };
                if !self.s1[c].is_finite() {
                    self.s1[c] = 0.0;
                }
                if !self.s2[c].is_finite() {
                    self.s2[c] = 0.0;
                }
                let denominator = 1.0 + self.k * self.g + self.g * self.g;
                let hp = if denominator.is_finite() && denominator > 0.0 {
                    (in_val - self.k * self.s1[c] - self.s2[c]) / denominator
                } else {
                    0.0
                };
                let bp = self.g * hp + self.s1[c];
                let lp = self.g * bp + self.s2[c];

                self.s1[c] = self.g * hp + bp;
                self.s2[c] = self.g * bp + lp;

                if c == 0 {
                    l[s] = if lp.is_finite() {
                        lp.clamp(-4.0, 4.0)
                    } else {
                        0.0
                    };
                } else {
                    r[s] = if lp.is_finite() {
                        lp.clamp(-4.0, 4.0)
                    } else {
                        0.0
                    };
                }
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Auto Filter state.
    pub fn audit_auto_filter(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.cutoff_base.is_finite()
            && (0.0..=1.0).contains(&self.cutoff_base)
            && self.res.is_finite()
            && (0.0..=1.0).contains(&self.res)
            && self.sens.is_finite()
            && (0.0..=1.0).contains(&self.sens)
            && self.env.is_finite()
            && self.env >= 0.0
            && self.attack.is_finite()
            && (0.0..=1.0).contains(&self.attack)
            && self.release.is_finite()
            && (0.0..=1.0).contains(&self.release)
            && self.g.is_finite()
            && self.g >= 0.0
            && self.k.is_finite()
            && self.s1.iter().chain(self.s2.iter()).all(|v| v.is_finite())
    }
}
