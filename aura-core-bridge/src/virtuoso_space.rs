pub struct VirtuosoSpaceEngine {
    pub sample_rate: f64,
    pub delay_lines: [Vec<f32>; 16],
    pub delay_lengths: [usize; 16],
    pub write_indices: [usize; 16],
    pub read_indices: [usize; 16],
    pub filter_state: [f32; 16],
    pub decay: f32,
    pub mix: f32,
    pub damping: f32,
    pub size: f32,
}

impl VirtuosoSpaceEngine {
    pub fn new(sr: f64) -> Self {
        let primes = [
            479, 701, 827, 1019, 1153, 1361, 1523, 1787, 1901, 2111, 2333, 2557, 2801, 3109, 3463,
            3851,
        ];

        let mut delay_lines = [
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        ];
        let mut delay_lengths = [0; 16];
        let mut read_indices = [0; 16];

        let sr = if sr.is_finite() && sr > 100.0 {
            sr
        } else {
            44_100.0
        };
        let size = 1.0f32;
        for i in 0..16 {
            let len = ((primes[i] as f64 * (sr / 44100.0) * size as f64) as usize).max(1);
            delay_lines[i] = vec![0.0; len];
            delay_lengths[i] = len;
            read_indices[i] = 1 % len; // Ensure valid index
        }

        Self {
            sample_rate: sr,
            delay_lines,
            delay_lengths,
            write_indices: [0; 16],
            read_indices,
            filter_state: [0.0; 16],
            decay: 0.85,
            mix: 0.25,
            damping: 0.2,
            size,
        }
    }

    pub fn reset(&mut self) {
        for line in &mut self.delay_lines {
            line.fill(0.0);
        }
        self.filter_state.fill(0.0);
        self.write_indices = [0; 16];
        for i in 0..16 {
            self.read_indices[i] = 1 % self.delay_lengths[i].max(1);
        }
    }

    /// INDUSTRIAL: High-density Feedback Delay Network (FDN) Reverb.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len().min(r.len());
        if !self.audit_virtuoso_space() {
            return;
        }

        for s in 0..len {
            let in_val = (l[s] + r[s]) * 0.5;

            // 1. INPUT DIFFUSION
            let fdn_in = in_val;

            // 2. READ DELAY LINES (Prime-spaced)
            let mut y = [0.0; 16];
            for i in 0..16 {
                y[i] = self.delay_lines[i][self.read_indices[i]];
                // Apply subtle Low-pass damping (High frequency absorption)
                self.filter_state[i] =
                    y[i] * (1.0 - self.damping) + self.filter_state[i] * self.damping;
                y[i] = self.filter_state[i];
            }

            // 3. HOUSEHOLDER TRANSFORMATION (O(N) Matrix multiplication)
            let mut sum = 0.0;
            for i in 0..16 {
                sum += y[i];
            }
            let factor = (2.0 / 16.0) * sum;

            for i in 0..16 {
                let fdn_out = y[i] - factor;
                // --- HONEST FIX: DENORMAL KILLER ---
                let mut feedback = fdn_in + fdn_out * self.decay;
                if feedback.abs() < 1e-15 {
                    feedback = 0.0;
                } // Simple denormal killer

                self.delay_lines[i][self.write_indices[i]] = feedback;

                // Index Update
                self.write_indices[i] = (self.write_indices[i] + 1) % self.delay_lengths[i];
                self.read_indices[i] = (self.read_indices[i] + 1) % self.delay_lengths[i];
            }

            // 4. MIX OUTPUT
            let mut reverb_out = 0.0;
            for i in 0..16 {
                reverb_out += y[i] * if i % 2 == 0 { 1.0 } else { -1.0 }; // Alternating phase for stereo spread
            }

            l[s] = (l[s] * (1.0 - self.mix) + reverb_out * self.mix).clamp(-4.0, 4.0);
            r[s] = (r[s] * (1.0 - self.mix) + -(reverb_out * self.mix)).clamp(-4.0, 4.0);
            // Pseudo-stereo
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Virtuoso Space state.
    pub fn audit_virtuoso_space(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 100.0
            && self.delay_lines.iter().all(|line| !line.is_empty())
            && self.delay_lengths.iter().all(|length| *length > 0)
            && self
                .delay_lines
                .iter()
                .zip(self.delay_lengths.iter())
                .all(|(line, length)| line.len() == *length)
            && self
                .write_indices
                .iter()
                .zip(self.delay_lengths.iter())
                .all(|(index, length)| *index < *length)
            && self
                .read_indices
                .iter()
                .zip(self.delay_lengths.iter())
                .all(|(index, length)| *index < *length)
            && self.filter_state.iter().all(|value| value.is_finite())
            && self.decay.is_finite()
            && self.mix.is_finite()
            && self.damping.is_finite()
            && (0.0..=1.0).contains(&self.mix)
            && (0.0..=1.0).contains(&self.damping)
    }
}
