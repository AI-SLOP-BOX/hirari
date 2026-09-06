pub struct SimpleSVF {
    pub g: f32,
    pub k: f32,
    pub s1: [f32; 2],
    pub s2: [f32; 2],
}

impl Default for SimpleSVF {
    fn default() -> Self {
        Self::new()
    }
}

impl SimpleSVF {
    pub fn new() -> Self {
        Self {
            g: 0.0,
            k: 1.0,
            s1: [0.0; 2],
            s2: [0.0; 2],
        }
    }

    pub fn set_parameters(&mut self, cutoff: f32, res: f32, sample_rate: f64) {
        let f = cutoff.clamp(20.0, 20000.0);
        self.g = (std::f64::consts::PI * f as f64 / sample_rate).tan() as f32;
        self.k = 2.0 - (res * 1.95); // Simplified resonance mapping
    }

    pub fn process_sample_lp(&mut self, in_val: f32, channel: usize) -> f32 {
        let c = channel;
        let hp =
            (in_val - self.k * self.s1[c] - self.s2[c]) / (1.0 + self.k * self.g + self.g * self.g);
        let bp = self.g * hp + self.s1[c];
        let lp = self.g * bp + self.s2[c];

        self.s1[c] = self.g * hp + bp;
        self.s2[c] = self.g * bp + lp;

        lp
    }
}

pub struct MultibandExciterEngine {
    pub sample_rate: f64,
    pub low_pass: SimpleSVF,
    pub mid_low_pass: SimpleSVF,
}

impl MultibandExciterEngine {
    pub fn new(sr: f64) -> Self {
        let sr = if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) {
            sr
        } else {
            48_000.0
        };
        let mut engine = Self {
            sample_rate: sr,
            low_pass: SimpleSVF::new(),
            mid_low_pass: SimpleSVF::new(),
        };
        engine.setup_crossover(200.0, 3000.0);
        engine
    }

    pub fn setup_crossover(&mut self, low_cut: f32, high_cut: f32) {
        self.low_pass
            .set_parameters(low_cut, 0.707, self.sample_rate);
        self.mid_low_pass
            .set_parameters(high_cut, 0.707, self.sample_rate);
    }

    fn apply_tube(&self, x: f32, warmth: f32) -> f32 {
        let b = warmth * 0.25; // Bias
        let driven = x + b;
        driven / (1.0 + driven.abs()) - (b / (1.0 + b.abs()))
    }

    fn apply_tape(&self, x: f32) -> f32 {
        let x_abs = x.abs();
        if x_abs < 1.0 {
            x * (1.5 - 0.5 * x * x)
        } else if x > 0.0 {
            1.0
        } else {
            -1.0
        }
    }

    fn apply_soft_clip(&self, x: f32) -> f32 {
        x.tanh()
    }

    /// INDUSTRIAL: Top-tier frequency-specific saturation.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if !self.audit_multiband_exciter() {
            return;
        }
        let len = l.len().min(r.len());

        for i in 0..len {
            let in_l = if l[i].is_finite() { l[i] } else { 0.0 };
            let in_r = if r[i].is_finite() { r[i] } else { 0.0 };

            // 1. FREQUENCY SPLITTING (LR-4 style approximation)
            let low_l = self.low_pass.process_sample_lp(in_l, 0);
            let low_r = self.low_pass.process_sample_lp(in_r, 1);

            let mid_high_l = in_l - low_l;
            let mid_high_r = in_r - low_r;

            let mid_l = self.mid_low_pass.process_sample_lp(mid_high_l, 0);
            let mid_r = self.mid_low_pass.process_sample_lp(mid_high_r, 1);

            let high_l = mid_high_l - mid_l;
            let high_r = mid_high_r - mid_r;

            // 2. APPLY SATURATION PER BAND
            // Low: Tube (warmth=0.2)
            let low_l_sat = self.apply_tube(low_l * 1.1, 0.2); // Drive=0.1 -> 1.1x
            let low_r_sat = self.apply_tube(low_r * 1.1, 0.2);

            // Mid: SoftClip (as FET)
            let mid_l_sat = self.apply_soft_clip(mid_l * 1.2); // Drive=0.2 -> 1.2x
            let mid_r_sat = self.apply_soft_clip(mid_r * 1.2);

            // High: Tape
            let high_l_sat = self.apply_tape(high_l * 1.4); // Drive=0.4 -> 1.4x
            let high_r_sat = self.apply_tape(high_r * 1.4);

            // 3. RECOMBINE
            l[i] = (low_l_sat + mid_l_sat + high_l_sat).clamp(-4.0, 4.0);
            r[i] = (low_r_sat + mid_r_sat + high_r_sat).clamp(-4.0, 4.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Multiband Exciter state.
    pub fn audit_multiband_exciter(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && [&self.low_pass, &self.mid_low_pass].iter().all(|f| {
                f.g.is_finite()
                    && f.k.is_finite()
                    && f.g >= 0.0
                    && (0.0..=2.0).contains(&f.k)
                    && f.s1.iter().chain(f.s2.iter()).all(|v| v.is_finite())
            })
    }
}
