pub struct LushReverbEngine {
    pub sample_rate: f64,
    pub delay_lines: [Vec<f32>; 8],
    pub write_indices: [usize; 8],
    pub filter_state: [f32; 8],
    pub feedback: f32,
    pub damping: f32,
}

impl LushReverbEngine {
    pub fn new(sr: f64) -> Self {
        let sr = if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) {
            sr
        } else {
            48_000.0
        };
        let lens = [1117, 1373, 1601, 2111, 2711, 3121, 3701, 4127];
        let delay_lines = [
            vec![0.0; lens[0]],
            vec![0.0; lens[1]],
            vec![0.0; lens[2]],
            vec![0.0; lens[3]],
            vec![0.0; lens[4]],
            vec![0.0; lens[5]],
            vec![0.0; lens[6]],
            vec![0.0; lens[7]],
        ];

        Self {
            sample_rate: sr,
            delay_lines,
            write_indices: [0; 8],
            filter_state: [0.0; 8],
            feedback: 0.85,
            damping: 0.2,
        }
    }

    pub fn reset(&mut self) {
        for dl in self.delay_lines.iter_mut() {
            dl.fill(0.0);
        }
        self.write_indices = [0; 8];
        self.filter_state = [0.0; 8];
    }

    /// INDUSTRIAL: Algorithmic Feedback Delay Network (FDN).
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if !self.audit_lush_reverb() {
            return;
        }
        let len = l.len().min(r.len());
        let scale = 0.3535f32; // 1 / sqrt(8)

        for s in 0..len {
            let mono_in = ((if l[s].is_finite() { l[s] } else { 0.0 })
                + (if r[s].is_finite() { r[s] } else { 0.0 }))
                * 0.5;

            let mut outputs = [0.0f32; 8];
            for i in 0..8 {
                outputs[i] = self.delay_lines[i][self.write_indices[i]];
            }

            // --- 8x8 HADAMARD MATRIX (ORTHOGONAL SCATTERING) ---
            let mut h = [0.0f32; 8];
            h[0] = outputs[0]
                + outputs[1]
                + outputs[2]
                + outputs[3]
                + outputs[4]
                + outputs[5]
                + outputs[6]
                + outputs[7];
            h[1] = outputs[0] - outputs[1] + outputs[2] - outputs[3] + outputs[4] - outputs[5]
                + outputs[6]
                - outputs[7];
            h[2] = outputs[0] + outputs[1] - outputs[2] - outputs[3] + outputs[4] + outputs[5]
                - outputs[6]
                - outputs[7];
            h[3] = outputs[0] - outputs[1] - outputs[2] + outputs[3] + outputs[4]
                - outputs[5]
                - outputs[6]
                + outputs[7];
            h[4] = outputs[0] + outputs[1] + outputs[2] + outputs[3]
                - outputs[4]
                - outputs[5]
                - outputs[6]
                - outputs[7];
            h[5] = outputs[0] - outputs[1] + outputs[2] - outputs[3] - outputs[4] + outputs[5]
                - outputs[6]
                + outputs[7];
            h[6] = outputs[0] + outputs[1] - outputs[2] - outputs[3] - outputs[4] - outputs[5]
                + outputs[6]
                + outputs[7];
            h[7] = outputs[0] - outputs[1] - outputs[2] + outputs[3] - outputs[4]
                + outputs[5]
                + outputs[6]
                - outputs[7];

            for i in 0..8 {
                let fb = h[i] * scale;
                self.filter_state[i] =
                    (1.0 - self.damping) * fb + self.damping * self.filter_state[i];

                // Denormal killer equivalent: check if very small and set to 0
                let mut next_val = mono_in + self.filter_state[i] * self.feedback;
                if next_val.abs() < 1e-15 {
                    next_val = 0.0;
                }

                self.delay_lines[i][self.write_indices[i]] = next_val;
                self.write_indices[i] = (self.write_indices[i] + 1) % self.delay_lines[i].len();
            }

            let wet_output = (h[0] + h[2] + h[4] + h[6]) * 0.125;
            l[s] = (l[s] + wet_output * 0.3).clamp(-4.0, 4.0);
            r[s] = (r[s] + wet_output * 0.3).clamp(-4.0, 4.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Lush Reverb state.
    pub fn audit_lush_reverb(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.feedback.is_finite()
            && (-0.999..=0.999).contains(&self.feedback)
            && self.damping.is_finite()
            && (0.0..=1.0).contains(&self.damping)
            && self
                .delay_lines
                .iter()
                .enumerate()
                .all(|(i, line)| !line.is_empty() && self.write_indices[i] < line.len())
            && self.filter_state.iter().all(|v| v.is_finite())
    }
}
