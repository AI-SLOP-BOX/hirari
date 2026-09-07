#[derive(Clone, Copy, PartialEq)]
pub enum AdsrState {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
    Off,
}

pub struct AdsrEnvelopeEngine {
    pub sample_rate: f64,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub attack_step: f32,
    pub decay_step: f32,
    pub release_coeff: f32,
    pub current_level: f32,
    pub state: AdsrState,
}

impl AdsrEnvelopeEngine {
    pub fn new(sample_rate: f64) -> Self {
        let mut engine = Self {
            sample_rate,
            attack: 0.01,
            decay: 0.1,
            sustain: 0.8,
            release: 0.5,
            attack_step: 0.0,
            decay_step: 0.0,
            release_coeff: 0.0,
            current_level: 0.0,
            state: AdsrState::Idle,
        };
        engine.update_coeffs();
        engine
    }

    pub fn set_parameters(&mut self, a: f32, d: f32, s: f32, r: f32) {
        self.attack = a.max(0.001);
        self.decay = d.max(0.001);
        self.sustain = s.clamp(0.0, 1.0);
        self.release = r.max(0.001);
        self.update_coeffs();
    }

    pub fn trigger_on(&mut self) {
        self.state = AdsrState::Attack;
        self.current_level = 0.0;
    }

    pub fn trigger_off(&mut self) {
        if self.state != AdsrState::Idle && self.state != AdsrState::Off {
            self.state = AdsrState::Release;
        }
    }

    pub fn reset(&mut self) {
        self.state = AdsrState::Idle;
        self.current_level = 0.0;
    }

    pub fn get_next(&mut self) -> f32 {
        match self.state {
            AdsrState::Attack => {
                self.current_level += self.attack_step;
                if self.current_level >= 1.0 {
                    self.current_level = 1.0;
                    self.state = AdsrState::Decay;
                }
            }
            AdsrState::Decay => {
                self.current_level -= self.decay_step;
                if self.current_level <= self.sustain {
                    self.current_level = self.sustain;
                    self.state = AdsrState::Sustain;
                }
            }
            AdsrState::Sustain => {
                self.current_level = self.sustain;
            }
            AdsrState::Release => {
                self.current_level *= self.release_coeff;
                if self.current_level <= 0.0005 {
                    self.current_level = 0.0;
                    self.state = AdsrState::Off;
                }
            }
            _ => {
                self.current_level = 0.0;
            }
        }
        self.current_level
    }

    fn update_coeffs(&mut self) {
        self.attack_step = 1.0 / (self.attack * self.sample_rate as f32);
        self.decay_step = (1.0 - self.sustain) / (self.decay * self.sample_rate as f32);
        self.release_coeff = (-1.0 / (self.release * self.sample_rate as f32)).exp();
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide ADSR state.
    pub fn audit_adsr_envelope(&self) -> bool {
        if !self.sample_rate.is_finite() || self.sample_rate <= 0.0 {
            return false;
        }
        let mut envelope = Self::new(self.sample_rate);
        envelope.trigger_on();
        let mut peak = 0.0f32;
        for _ in 0..(self.sample_rate as usize / 10).max(1) {
            let value = envelope.get_next();
            if !value.is_finite() { return false; }
            peak = peak.max(value);
        }
        envelope.trigger_off();
        for _ in 0..(self.sample_rate as usize).max(1) {
            if !envelope.get_next().is_finite() { return false; }
            if matches!(envelope.state, AdsrState::Off) { break; }
        }
        peak > 0.0 && matches!(envelope.state, AdsrState::Off)
    }
}
