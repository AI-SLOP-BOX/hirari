#[derive(Clone, Copy, PartialEq)]
pub enum Waveform {
    Saw,
    Square,
    Triangle,
    Sine,
}

pub struct VaOscillatorEngine {
    pub sample_rate: f64,
    pub phase: f64,
    pub freq: f64,
    pub increment: f64,
    pub waveform: Waveform,
}

impl VaOscillatorEngine {
    pub fn new(sample_rate: f64) -> Self {
        let sample_rate = if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) { sample_rate } else { 48_000.0 };
        let mut engine = Self {
            sample_rate,
            phase: 0.0,
            freq: 440.0,
            increment: 0.0,
            waveform: Waveform::Saw,
        };
        engine.update_increment();
        engine
    }

    pub fn set_frequency(&mut self, freq: f64) {
        if freq.is_finite() { self.freq = freq.clamp(0.0, self.sample_rate * 0.49); }
        self.update_increment();
    }

    pub fn set_waveform(&mut self, wave: Waveform) {
        self.waveform = wave;
    }

    /// INDUSTRIAL: Renders the next sample of the chosen waveform.
    pub fn process(&mut self) -> f32 {
        if !self.audit_va_oscillator() { return 0.0; }
        let mut out: f32;
        let p = self.phase;
        let dt = self.increment;

        match self.waveform {
            Waveform::Sine => {
                out = (p * 2.0 * std::f64::consts::PI).sin() as f32;
            }
            Waveform::Saw => {
                out = (2.0 * p - 1.0) as f32;
                out -= self.bleach(p, dt) as f32; // PolyBLEP Correction
            }
            Waveform::Square => {
                out = if p < 0.5 { 1.0 } else { -1.0 };
                out += self.bleach(p, dt) as f32;
                out -= self.bleach((p + 0.5) % 1.0, dt) as f32;
            }
            Waveform::Triangle => {
                out = (4.0 * (p - 0.5).abs() - 1.0) as f32;
            }
        }

        self.phase = (self.phase + dt).rem_euclid(1.0);

        if out.is_finite() { out.clamp(-1.5, 1.5) } else { 0.0 }
    }

    fn bleach(&self, mut t: f64, dt: f64) -> f64 {
        if t < dt {
            t /= dt;
            t + t - t * t - 1.0
        } else if t > 1.0 - dt {
            t = (t - 1.0) / dt;
            t * t + t + t + 1.0
        } else {
            0.0
        }
    }

    fn update_increment(&mut self) {
        self.increment = if self.sample_rate.is_finite() && self.sample_rate > 0.0 { self.freq / self.sample_rate } else { 0.0 };
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide VA state.
    pub fn audit_va_oscillator(&self) -> bool {
        self.sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.phase.is_finite() && (0.0..1.0).contains(&self.phase)
            && self.freq.is_finite() && (0.0..=self.sample_rate * 0.49).contains(&self.freq)
            && self.increment.is_finite() && (0.0..=0.49).contains(&self.increment)
    }
}
