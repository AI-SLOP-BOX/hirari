use crate::wavetable_oscillator::WavetableOscillatorEngine;

pub struct SimpleAdsr {
    pub level: f32,
    pub target: f32,
}

impl Default for SimpleAdsr {
    fn default() -> Self {
        Self::new()
    }
}

impl SimpleAdsr {
    pub fn new() -> Self {
        Self {
            level: 0.0,
            target: 0.0,
        }
    }
    pub fn get_next(&mut self) -> f32 {
        self.level += (self.target - self.level) * 0.001;
        self.level
    }
    pub fn trigger(&mut self) {
        self.target = 1.0;
    }
    pub fn release(&mut self) {
        self.target = 0.0;
    }
}

pub struct AuraWavetableSynthEngine {
    pub sample_rate: f64,
    pub osc: WavetableOscillatorEngine,
    pub aeg: SimpleAdsr,
    pub feg: SimpleAdsr,

    pub morph_pos: f32,
    pub cutoff1: f32,
    pub res1: f32,
    pub cutoff2: f32,
    pub res2: f32,
    pub drive: f32,
    pub feedback: f32,

    pub f1_z1: f32,
    pub f2_z1: f32,
    pub hpf_z1: f32,
    pub last_in: f32,
    pub last_f2_out: f32,
    pub velocity: f32,
}

impl AuraWavetableSynthEngine {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            osc: WavetableOscillatorEngine::new(sample_rate),
            aeg: SimpleAdsr::new(),
            feg: SimpleAdsr::new(),
            morph_pos: 0.5,
            cutoff1: 1000.0,
            res1: 0.2,
            cutoff2: 2000.0,
            res2: 0.1,
            drive: 1.0,
            feedback: 0.1,
            f1_z1: 0.0,
            f2_z1: 0.0,
            hpf_z1: 0.0,
            last_in: 0.0,
            last_f2_out: 0.0,
            velocity: 0.0,
        }
    }

    pub fn note_on(&mut self, freq: f64, vel: f32) {
        if !freq.is_finite() || !(1.0..=20_000.0).contains(&freq) || !vel.is_finite() { return; }
        self.osc.set_frequency(freq);
        self.velocity = vel.clamp(0.0, 1.0);
        self.aeg.trigger();
        self.feg.trigger();
    }

    /// INDUSTRIAL: Processes an audio block with feedback routing and WaveShaping.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if l.len() != r.len() || !self.sample_rate.is_finite() || !(8_000.0..=384_000.0).contains(&self.sample_rate) { return; }
        let n = l.len();
        let sr = self.sample_rate as f32;

        for i in 0..n {
            // --- 1. OSCILLATOR & MORPH ---
            let raw = self.osc.process(self.morph_pos);

            // --- 2. FILTER SECTION (F1 -> WS -> F2) ---
            // F1: Low-pass with Feedback
            let f1_in = raw + (self.last_f2_out * self.feedback);

            let f1 = 1.5 * (std::f32::consts::PI * self.cutoff1 / sr).sin();
            let q1 = 1.0 - self.res1;
            self.f1_z1 = self.f1_z1 + f1 * (f1_in - self.f1_z1 + q1 * (f1_in - self.f1_z1));
            let f1_out = self.f1_z1;

            // WS: WaveShaper (Saturation)
            let ws_out = (f1_out * self.drive).tanh();

            // F2: Secondary Filter (Multi-mode)
            let f2 = 1.5 * (std::f32::consts::PI * self.cutoff2 / sr).sin();
            let q2 = 1.0 - self.res2;
            self.f2_z1 = self.f2_z1 + f2 * (ws_out - self.f2_z1 + q2 * (ws_out - self.f2_z1));
            let f2_out = self.f2_z1;
            self.last_f2_out = f2_out;

            // --- 3. AMP ENVELOPE (AEG) ---
            let env = self.aeg.get_next();
            let final_sample = f2_out * env * self.velocity;

            // --- 4. MASTER HPF (Logic Pro Style) ---
            let alpha = 1.0 / (1.0 + 2.0 * std::f32::consts::PI * 20.0 / sr);
            let hpf_out = alpha * (self.hpf_z1 + final_sample - self.last_in);
            self.last_in = final_sample;
            self.hpf_z1 = hpf_out;

            l[i] += hpf_out;
            r[i] += hpf_out;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Aura Wavetable Synth state.
    pub fn audit_aura_wavetable_synth(&self) -> bool {
        self.sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && [self.morph_pos, self.cutoff1, self.res1, self.cutoff2, self.res2, self.drive, self.feedback, self.velocity, self.f1_z1, self.f2_z1, self.hpf_z1, self.last_in, self.last_f2_out].iter().all(|v| v.is_finite())
            && (0.0..=1.0).contains(&self.morph_pos) && (0.0..=1.0).contains(&self.res1) && (0.0..=1.0).contains(&self.res2) && (0.0..=1.0).contains(&self.feedback) && self.drive >= 0.0
    }
}
