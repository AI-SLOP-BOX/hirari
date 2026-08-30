pub enum SvfType {
    LowPass,
    HighPass,
    BandPass,
    Bell,
    Notch,
    LowShelf,
    HighShelf,
}

pub struct StateVariableFilterEngine {
    pub sample_rate: f64,
    pub filter_type: SvfType,
    pub g: f32,
    pub k: f32,
    pub gain: f32,
    pub a1: f32,
    pub a2: f32,
    pub a3: f32,
    pub s1: [f32; 2], // Max stereo
    pub s2: [f32; 2],
}

impl StateVariableFilterEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            filter_type: SvfType::LowPass,
            g: 0.0,
            k: 0.0,
            gain: 1.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            s1: [0.0; 2],
            s2: [0.0; 2],
        }
    }

    pub fn reset(&mut self) {
        self.s1 = [0.0; 2];
        self.s2 = [0.0; 2];
    }

    pub fn set_params(&mut self, freq: f32, gain_db: f32, q: f32) {
        if !self.sample_rate.is_finite() || self.sample_rate <= 100.0 {
            return;
        }
        let freq = if freq.is_finite() {
            freq.clamp(5.0, self.sample_rate as f32 * 0.49)
        } else {
            1000.0
        };
        let q = if q.is_finite() {
            q.clamp(0.05, 100.0)
        } else {
            0.707
        };
        let gain_db = if gain_db.is_finite() {
            gain_db.clamp(-48.0, 48.0)
        } else {
            0.0
        };
        let g = (std::f32::consts::PI * freq / self.sample_rate as f32)
            .tan()
            .clamp(0.0, 100.0);
        let k = 1.0 / q;
        let a = 10.0f32.powf(gain_db / 40.0); // Half-gain for shelf sum logic

        self.g = g;
        self.k = k;
        self.gain = a;
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    /// INDUSTRIAL: High-performance buffer sum.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len().min(r.len());
        if !self.audit_state_variable_filter() {
            return;
        }

        let mut process_channel = |data: &mut [f32], ch_idx: usize| {
            let mut s1 = self.s1[ch_idx];
            let mut s2 = self.s2[ch_idx];

            for i in 0..len {
                let x = data[i];
                let v3 = x - s2;
                let v1 = self.a1 * s1 + self.a2 * v3;
                let v2 = s2 + self.a2 * s1 + self.a3 * v3;

                s1 = 2.0 * v1 - s1;
                s2 = 2.0 * v2 - s2;

                // Pultec-style Parallel Summation for Shelves
                data[i] = match self.filter_type {
                    SvfType::LowShelf => x + self.gain * v2,
                    SvfType::HighShelf => x + self.gain * (x - self.k * v1 - v2),
                    SvfType::LowPass => v2,
                    SvfType::HighPass => x - self.k * v1 - v2,
                    SvfType::BandPass => v1,
                    _ => v1, // Fallback
                };
                if !data[i].is_finite() {
                    data[i] = 0.0;
                    s1 = 0.0;
                    s2 = 0.0;
                }
            }

            self.s1[ch_idx] = s1;
            self.s2[ch_idx] = s2;
        };

        process_channel(l, 0);
        process_channel(r, 1);
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide State Variable Filter state.
    pub fn audit_state_variable_filter(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 100.0
            && [self.g, self.k, self.gain, self.a1, self.a2, self.a3]
                .iter()
                .all(|value| value.is_finite())
            && self
                .s1
                .iter()
                .chain(self.s2.iter())
                .all(|value| value.is_finite())
    }
}
