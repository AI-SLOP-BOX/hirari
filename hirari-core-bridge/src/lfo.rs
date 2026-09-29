#[derive(Clone, Copy, PartialEq)]
pub enum Waveform {
    Sine,
    Triangle,
    Saw,
    Square,
}

pub struct LfoEngine {
    pub sample_rate: f32,
    pub phase: f64,
    pub phase_inc: f64,
}

impl LfoEngine {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            phase: 0.0,
            phase_inc: 0.0,
        }
    }

    pub fn set_frequency(&mut self, freq: f32) {
        let safe_freq = if freq.is_finite() && freq >= 0.0 {
            freq
        } else {
            0.0
        };
        let safe_sr = if self.sample_rate.is_finite() && self.sample_rate > 0.0 {
            self.sample_rate
        } else {
            44100.0
        };
        self.phase_inc = (safe_freq as f64 / safe_sr as f64).min(0.5); // Clamp below Nyquist
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return;
        }
        let frequency = self.phase_inc * self.sample_rate as f64;
        self.sample_rate = sample_rate;
        self.phase_inc = frequency / sample_rate as f64;
    }

    /// INDUSTRIAL: Renders the next sample of modulation.
    pub fn process(&mut self, wave: Waveform) -> f32 {
        let phase = self.phase - self.phase.floor();
        let step = if self.phase_inc.is_finite() && self.phase_inc >= 0.0 {
            self.phase_inc
        } else {
            0.0
        };
        let output = match wave {
            Waveform::Sine => (2.0 * std::f64::consts::PI * phase).sin() as f32,
            Waveform::Triangle => (1.0 - 4.0 * (phase - 0.5).abs()) as f32,
            Waveform::Saw => (2.0 * phase - 1.0) as f32,
            Waveform::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        };
        self.phase += step;
        self.phase = self.phase.rem_euclid(1.0);
        output
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide LFO state.
    pub fn audit_lfo(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && self.phase.is_finite()
            && (0.0..1.0).contains(&self.phase)
            && self.phase_inc.is_finite()
            && (0.0..=0.5).contains(&self.phase_inc)
    }
}

#[cfg(test)]
mod tests {
    use super::LfoEngine;

    #[test]
    fn lfo_audit_rejects_non_finite_or_out_of_range_state() {
        let mut lfo = LfoEngine::new(48_000.0);
        lfo.set_frequency(1_000.0);
        assert!(lfo.audit_lfo());
        lfo.phase = 1.0;
        assert!(!lfo.audit_lfo());
        lfo.phase = 0.0;
        lfo.phase_inc = f64::NAN;
        assert!(!lfo.audit_lfo());
    }
}
