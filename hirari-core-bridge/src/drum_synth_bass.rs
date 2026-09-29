pub struct DrumSynthBassEngine {
    pub sample_rate: f64,
    pub phase: f64,
    pub env_pos: f64,
    pub p_start: f32,
    pub p_decay: f32,
    pub p_sat: f32,
    pub pitch_env: f32,
    pub pitch_drop_coef: f32,
    pub is_active: bool,
    pub lut: [f32; 4096],
}

impl DrumSynthBassEngine {
    pub fn new(sample_rate: f64) -> Self {
        let sample_rate = if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate)
        {
            sample_rate
        } else {
            48_000.0
        };
        let mut lut = [0.0f32; 4096];
        for i in 0..4096 {
            lut[i] = (2.0 * std::f64::consts::PI * i as f64 / 4096.0).sin() as f32;
        }
        Self {
            sample_rate,
            phase: 0.0,
            env_pos: 0.0,
            p_start: 150.0,
            p_decay: 0.5,
            p_sat: 1.2,
            pitch_env: 1.0,
            pitch_drop_coef: 1.0,
            is_active: false,
            lut,
        }
    }

    pub fn trigger(&mut self, pitch_start: f32, decay: f32, saturation: f32) {
        if !pitch_start.is_finite() || !decay.is_finite() || !saturation.is_finite() {
            return;
        }
        self.phase = 0.0;
        self.env_pos = 0.0;
        self.p_start = pitch_start.clamp(0.0, 20_000.0);
        self.p_decay = decay.clamp(0.001, 60.0);
        self.p_sat = saturation.clamp(0.0, 16.0);

        self.pitch_env = 1.0;
        self.pitch_drop_coef = (-22.0 / self.sample_rate).exp() as f32;

        self.is_active = true;
    }

    /// INDUSTRIAL: Processes an audio block with SOTA drum synthesis.
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        if !self.is_active {
            return;
        }

        let inv_sr = 1.0 / self.sample_rate as f32;
        let d_rate = 1.0 / (self.p_decay + 0.001);

        let k_lut_size = 4096;
        let k_lut_mask = k_lut_size - 1;

        if !self.audit_drum_synth_bass() {
            return;
        }
        let num_frames = l.len().min(r.len());

        for i in 0..num_frames {
            let env = 1.0 - (self.env_pos as f32 * d_rate);
            if env <= 0.0 {
                self.is_active = false;
                break;
            }

            // Exponential Pitch Drop
            let freq = 42.0 + self.p_start * self.pitch_env;
            self.pitch_env *= self.pitch_drop_coef;

            // ULTRA-FAST LUT LOOKUP
            let phase_idx = self.phase * (k_lut_size as f64 / (2.0 * std::f64::consts::PI));
            let i1 = (phase_idx as usize) & k_lut_mask;
            let i2 = (i1 + 1) & k_lut_mask;
            let frac = (phase_idx - phase_idx.floor()) as f32;
            let s = self.lut[i1] * (1.0 - frac) + self.lut[i2] * frac;

            // RATIONAL SATURATION
            let raw = s * env;
            let x = (raw * self.p_sat).clamp(-3.0, 3.0);
            let sat = x * (27.0 + x * x) / (27.0 + 9.0 * x * x);

            l[i] = ((if l[i].is_finite() { l[i] } else { 0.0 }) + sat).clamp(-4.0, 4.0);
            r[i] = ((if r[i].is_finite() { r[i] } else { 0.0 }) + sat).clamp(-4.0, 4.0);

            self.phase += 2.0 * std::f64::consts::PI * freq as f64 * inv_sr as f64;
            if self.phase > 2.0 * std::f64::consts::PI {
                self.phase -= 2.0 * std::f64::consts::PI;
            }
            self.env_pos += inv_sr as f64;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Drum Synth state.
    pub fn audit_drum_synth_bass(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.phase.is_finite()
            && self.env_pos.is_finite()
            && self.env_pos >= 0.0
            && self.p_start.is_finite()
            && (0.0..=20_000.0).contains(&self.p_start)
            && self.p_decay.is_finite()
            && self.p_decay > 0.0
            && self.p_sat.is_finite()
            && (0.0..=16.0).contains(&self.p_sat)
            && self.pitch_env.is_finite()
            && self.pitch_drop_coef.is_finite()
    }
}
