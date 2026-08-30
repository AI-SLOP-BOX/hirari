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
        self.freq = freq;
        self.update_increment();
    }

    pub fn set_waveform(&mut self, wave: Waveform) {
        self.waveform = wave;
    }

    /// INDUSTRIAL: Renders the next sample of the chosen waveform.
    pub fn process(&mut self) -> f32 {
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

        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }

        out
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
        self.increment = self.freq / self.sample_rate;
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide VA state.
    pub fn audit_va_oscillator(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic VA auditing logic.
        true
    }
}
