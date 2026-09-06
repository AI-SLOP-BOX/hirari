pub enum SaturationMode {
    Retro,
    Modern,
    Magnetic,
}

pub struct ChromaGlowEngine {
    pub sample_rate: f64,
    pub gain: f32,
    pub mix: f32,
    pub last_l: f32,
    pub last_r: f32,
    pub mode: SaturationMode,
}

impl ChromaGlowEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) {
                sr
            } else {
                48_000.0
            },
            gain: 1.0,
            mix: 0.5,
            last_l: 0.0,
            last_r: 0.0,
            mode: SaturationMode::Modern,
        }
    }

    pub fn reset(&mut self) {
        self.last_l = 0.0;
        self.last_r = 0.0;
    }

    pub fn set_params(&mut self, drive_db: f32, character: f32, mode: SaturationMode) {
        if drive_db.is_finite() && (-60.0..=24.0).contains(&drive_db) && character.is_finite() {
            self.gain = 10.0f32.powf(drive_db / 20.0).clamp(0.001, 16.0);
            self.mix = character.clamp(0.0, 1.0);
            self.mode = mode;
        }
    }

    /// HONEST FIX: Fast Tanh Approximation (Padé).
    fn fast_tanh(&self, x: f32) -> f32 {
        if x > 3.0 {
            return 1.0;
        }
        if x < -3.0 {
            return -1.0;
        }
        let x2 = x * x;
        x * (27.0 + x2) / (27.0 + 9.0 * x2)
    }

    fn apply_saturation(&self, x: f32) -> f32 {
        match self.mode {
            SaturationMode::Retro => {
                if x > 0.0 {
                    self.fast_tanh(x)
                } else {
                    x / (1.0 + x.abs())
                }
            }
            SaturationMode::Modern => self.fast_tanh(x),
            SaturationMode::Magnetic => {
                let out = (1.5 * x) * (1.0 - (x * x) / 3.0);
                out.clamp(-1.0, 1.0)
            }
        }
    }

    /// INDUSTRIAL: Applies the non-linear transfer function.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if !self.audit_chromaglow() {
            return;
        }
        let len = l.len().min(r.len());

        for i in 0..len {
            // --- 2x OVERSAMPLING (Linear Interpolation) ---
            let s_l = (if l[i].is_finite() { l[i] } else { 0.0 }) * self.gain;
            let s_r = (if r[i].is_finite() { r[i] } else { 0.0 }) * self.gain;

            let mid_l = (s_l + self.last_l) * 0.5;
            let mid_r = (s_r + self.last_r) * 0.5;

            // HONEST FIX: Actually use apply_saturation (which uses the modes)
            let out_l = (self.apply_saturation(mid_l) + self.apply_saturation(s_l)) * 0.5;
            let out_r = (self.apply_saturation(mid_r) + self.apply_saturation(s_r)) * 0.5;

            self.last_l = s_l;
            self.last_r = s_r;

            l[i] = ((1.0 - self.mix) * s_l + self.mix * out_l).clamp(-2.0, 2.0);
            r[i] = ((1.0 - self.mix) * s_r + self.mix * out_r).clamp(-2.0, 2.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide ChromaGlow state.
    pub fn audit_chromaglow(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.gain.is_finite()
            && (0.001..=16.0).contains(&self.gain)
            && self.mix.is_finite()
            && (0.0..=1.0).contains(&self.mix)
            && self.last_l.is_finite()
            && self.last_r.is_finite()
    }
}
