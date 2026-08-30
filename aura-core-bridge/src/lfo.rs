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

    /// INDUSTRIAL: Renders the next sample of modulation.
    pub fn process(&mut self, wave: Waveform) -> f32 {
        let step = if self.phase_inc.is_finite() && self.phase_inc >= 0.0 {
            self.phase_inc
        } else {
            0.0
        };
        self.phase += step;
        if self.phase >= 1.0 {
            self.phase = self.phase.rem_euclid(1.0);
        }

        match wave {
            Waveform::Sine => (2.0 * std::f64::consts::PI * self.phase).sin() as f32,
            Waveform::Triangle => {
                let p = self.phase as f32;
                if p < 0.5 {
                    4.0 * p - 1.0
                } else {
                    3.0 - 4.0 * p
                }
            }
            Waveform::Saw => (2.0 * self.phase - 1.0) as f32,
            Waveform::Square => {
                if self.phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
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
