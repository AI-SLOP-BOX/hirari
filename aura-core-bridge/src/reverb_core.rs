pub struct TptOnePole {
    pub s: f32,
    pub g: f32,
}

impl Default for TptOnePole {
    fn default() -> Self {
        Self::new()
    }
}

impl TptOnePole {
    pub fn new() -> Self {
        Self { s: 0.0, g: 0.5 }
    }

    pub fn set_cutoff(&mut self, fc: f32, sr: f32) {
        if !fc.is_finite() || !sr.is_finite() || sr <= 100.0 {
            return;
        }
        let fc = fc.clamp(5.0, sr * 0.45);
        let wd = 2.0 * std::f32::consts::PI * fc;
        let t = 1.0 / sr;
        let wa = (2.0 / t) * (wd * t / 2.0).tan();
        self.g = (wa * t / 2.0) / (1.0 + (wa * t / 2.0));
    }

    pub fn process_lp(&mut self, x: f32) -> f32 {
        let v = (x - self.s) * self.g;
        let y = v + self.s;
        self.s = y + v;
        y
    }
}

pub struct ReverbCoreEngine {
    pub sample_rate: f64,
    pub delays: [Vec<f32>; 8],
    pub delay_lengths: [usize; 8],
    pub read_pos: [usize; 8],
    pub gains: [f32; 8],
    pub damping: [TptOnePole; 8],
}

impl ReverbCoreEngine {
    pub fn new(sr: f64) -> Self {
        let sr = if sr.is_finite() && sr > 100.0 {
            sr
        } else {
            44_100.0
        };
        let delays = [
            vec![0.0; 1032],
            vec![0.0; 1154],
            vec![0.0; 1322],
            vec![0.0; 1460],
            vec![0.0; 1602],
            vec![0.0; 1824],
            vec![0.0; 2000],
            vec![0.0; 2334],
        ];

        let mut damping = [
            TptOnePole::new(),
            TptOnePole::new(),
            TptOnePole::new(),
            TptOnePole::new(),
            TptOnePole::new(),
            TptOnePole::new(),
            TptOnePole::new(),
            TptOnePole::new(),
        ];

        for i in 0..8 {
            damping[i].set_cutoff(12000.0, sr as f32);
        }

        let mut engine = Self {
            sample_rate: sr,
            delays,
            delay_lengths: [1031, 1153, 1321, 1459, 1601, 1823, 1999, 2333],
            read_pos: [0; 8],
            gains: [0.0; 8],
            damping,
        };

        engine.set_decay(2.4);
        engine
    }

    pub fn set_decay(&mut self, t60: f32) {
        if !t60.is_finite()
            || t60 <= 0.0
            || !self.sample_rate.is_finite()
            || self.sample_rate <= 0.0
        {
            return;
        }
        for i in 0..8 {
            self.gains[i] =
                10.0f32.powf(-3.0 * self.delay_lengths[i] as f32 / (t60 * self.sample_rate as f32));
        }
    }

    pub fn reset(&mut self) {
        for d in &mut self.delays {
            d.fill(0.0);
        }
        self.read_pos = [0; 8];
    }

    /// INDUSTRIAL: High-quality algorithmic reverb.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len().min(r.len());
        if !self.audit_reverb_core() {
            return;
        }

        for s in 0..len {
            let in_val = (l[s] + r[s]) * 0.4; // Input scale

            // 1. Gather Latency-aligned Samples
            let mut x = [0.0; 8];
            for i in 0..8 {
                x[i] = self.delays[i][self.read_pos[i]];
            }

            // 2. MIXING MATRIX (Householder)
            let mut sum = 0.0;
            for i in 0..8 {
                sum += x[i];
            }
            let factor = 2.0 / 8.0;
            let mut y = [0.0; 8];
            for i in 0..8 {
                y[i] = x[i] - (sum * factor);
            }

            // 3. Feedback Loop with TPT Damping
            for i in 0..8 {
                let feedback = self.damping[i].process_lp(y[i] * self.gains[i]);
                self.delays[i][self.read_pos[i]] = in_val + feedback;
                self.read_pos[i] = (self.read_pos[i] + 1) % self.delay_lengths[i];
            }

            // 4. Output Tap (Phase Decorrelated)
            let out_l = (y[0] + y[2] + y[4] + y[6]) * 0.25;
            let out_r = (y[1] + y[3] + y[5] + y[7]) * 0.25;

            l[s] = (l[s] * 0.7 + out_l * 0.3).clamp(-4.0, 4.0);
            r[s] = (r[s] * 0.7 + out_r * 0.3).clamp(-4.0, 4.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Reverb Core state.
    pub fn audit_reverb_core(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 100.0
            && self
                .delays
                .iter()
                .zip(self.delay_lengths.iter())
                .all(|(delay, length)| *length > 0 && delay.len() == *length)
            && self
                .read_pos
                .iter()
                .zip(self.delay_lengths.iter())
                .all(|(pos, length)| *pos < *length)
            && self.gains.iter().all(|gain| gain.is_finite())
            && self.damping.iter().all(|filter| {
                filter.s.is_finite() && filter.g.is_finite() && (0.0..=1.0).contains(&filter.g)
            })
    }
}
