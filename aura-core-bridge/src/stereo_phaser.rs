pub struct StereoPhaserEngine {
    pub sample_rate: f64,
    pub lfo_phase: f64,
    pub rate: f32,
    pub mix: f32,
    pub feedback: f32,
    pub filter_state_l: [f32; 4],
    pub filter_state_r: [f32; 4],
    pub last_out_l: f32,
    pub last_out_r: f32,
}

impl StereoPhaserEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            lfo_phase: 0.0,
            rate: 0.5,
            mix: 0.5,
            feedback: 0.3,
            filter_state_l: [0.0; 4],
            filter_state_r: [0.0; 4],
            last_out_l: 0.0,
            last_out_r: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.filter_state_l = [0.0; 4];
        self.filter_state_r = [0.0; 4];
        self.last_out_l = 0.0;
        self.last_out_r = 0.0;
    }

    pub fn set_mix(&mut self, m: f32) {
        self.mix = m;
    }

    pub fn set_rate(&mut self, r: f32) {
        self.rate = r;
    }

    pub fn set_feedback(&mut self, f: f32) {
        self.feedback = f;
    }

    /// INDUSTRIAL: Modulates phase cancellation points over time.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len().min(r.len());
        if !self.sample_rate.is_finite()
            || self.sample_rate <= 100.0
            || !self.rate.is_finite()
            || !self.mix.is_finite()
            || !self.feedback.is_finite()
        {
            return;
        }
        self.mix = self.mix.clamp(0.0, 1.0);
        self.feedback = self.feedback.clamp(-0.99, 0.99);

        for s in 0..len {
            // 1. Stereo LFO
            self.lfo_phase += self.rate as f64 / self.sample_rate;
            if self.lfo_phase >= 1.0 {
                self.lfo_phase -= 1.0;
            }

            let lfo_l = 0.5 + 0.5 * (2.0 * std::f64::consts::PI * self.lfo_phase).sin();
            let lfo_r = 0.5
                + 0.5
                    * (2.0 * std::f64::consts::PI * self.lfo_phase + 0.5 * std::f64::consts::PI)
                        .sin();

            // 2. Filter Update & Filter Process
            for c in 0..2 {
                let lfo_val = if c == 0 { lfo_l } else { lfo_r };
                // Linear interpolation equivalent in Rust
                let freq = 500.0 + (4000.0 - 500.0) * lfo_val as f32;
                let g = (freq - self.sample_rate as f32) / (freq + self.sample_rate as f32); // Simplified all-pass coeff

                let in_val = if c == 0 {
                    l[s] + self.feedback * self.last_out_l
                } else {
                    r[s] + self.feedback * self.last_out_r
                };

                // 4-Stage All-pass cascade
                let mut y = in_val;
                for stage in 0..4 {
                    let out = g * y
                        + if c == 0 {
                            self.filter_state_l[stage]
                        } else {
                            self.filter_state_r[stage]
                        };
                    if c == 0 {
                        self.filter_state_l[stage] = y - g * out;
                    } else {
                        self.filter_state_r[stage] = y - g * out;
                    }
                    y = out;
                }

                if c == 0 {
                    self.last_out_l = y;
                    l[s] = ((if l[s].is_finite() { l[s] } else { 0.0 }) * (1.0 - self.mix)
                        + y * self.mix)
                        .clamp(-4.0, 4.0);
                } else {
                    self.last_out_r = y;
                    r[s] = ((if r[s].is_finite() { r[s] } else { 0.0 }) * (1.0 - self.mix)
                        + y * self.mix)
                        .clamp(-4.0, 4.0);
                }
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Stereo Phaser state.
    pub fn audit_stereo_phaser(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 100.0
            && self.lfo_phase.is_finite()
            && self.rate.is_finite()
            && self.mix.is_finite()
            && self.feedback.is_finite()
            && (0.0..=1.0).contains(&self.mix)
            && (-0.99..=0.99).contains(&self.feedback)
            && self
                .filter_state_l
                .iter()
                .chain(self.filter_state_r.iter())
                .all(|v| v.is_finite())
    }
}
