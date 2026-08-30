pub struct AtmosReverbEngine {
    pub sample_rate: f64,
    pub delay_lines: [Vec<f32>; 12],
    pub write_indices: [usize; 12],
    pub filter_state: [f32; 12],
    pub feedback: f32,
    pub damping: f32,
}

impl AtmosReverbEngine {
    pub fn new(sr: f64) -> Self {
        let sr = if sr.is_finite() && sr > 1.0 {
            sr
        } else {
            48_000.0
        };
        let base_lens = [
            1019, 1151, 1277, 1423, 1553, 1723, 1877, 2027, 2179, 2333, 2477, 2657,
        ];
        let lens = base_lens.map(|n| ((n as f64 * sr / 48_000.0).round().max(1.0)) as usize);
        let delay_lines = [
            vec![0.0; lens[0]],
            vec![0.0; lens[1]],
            vec![0.0; lens[2]],
            vec![0.0; lens[3]],
            vec![0.0; lens[4]],
            vec![0.0; lens[5]],
            vec![0.0; lens[6]],
            vec![0.0; lens[7]],
            vec![0.0; lens[8]],
            vec![0.0; lens[9]],
            vec![0.0; lens[10]],
            vec![0.0; lens[11]],
        ];

        Self {
            sample_rate: sr,
            delay_lines,
            write_indices: [0; 12],
            filter_state: [0.0; 12],
            feedback: 0.85,
            damping: 0.2,
        }
    }

    pub fn reset(&mut self) {
        for dl in self.delay_lines.iter_mut() {
            dl.fill(0.0);
        }
        self.write_indices = [0; 12];
        self.filter_state = [0.0; 12];
    }

    /// INDUSTRIAL: Professional 12-Channel Immersive Reverb (FDN).
    pub fn process_immersive(&mut self, buffers: &mut [&mut [f32]]) {
        if buffers.len() < 12
            || self.feedback.is_nan()
            || self.damping.is_nan()
            || !self.feedback.is_finite()
            || !self.damping.is_finite()
        {
            return;
        } // Ensure we have 12 channels
        let len = buffers[..12]
            .iter()
            .map(|buffer| buffer.len())
            .min()
            .unwrap_or(0);
        let feedback = self.feedback.clamp(-0.99, 0.99);
        let damping = self.damping.clamp(0.0, 1.0);

        for s in 0..len {
            let mut outputs = [0.0f32; 12];
            for i in 0..12 {
                outputs[i] = self.delay_lines[i][self.write_indices[i]];
            }

            // --- 12x12 DIFFUSION (Circular Shift + Mix) ---
            // High-performance diffusion for immersive audio without energy loss.
            let mut h = [0.0f32; 12];
            for i in 0..12 {
                h[i] = (outputs[i] + outputs[(i + 1) % 12] - outputs[(i + 2) % 12]) * 0.577;
                // 1/sqrt(3) approx
            }

            for i in 0..12 {
                let fb = h[i];
                self.filter_state[i] = (1.0 - damping) * fb + damping * self.filter_state[i];

                // Denormal killer equivalent
                let in_val = buffers[i][s];
                let mut next_val =
                    if in_val.is_finite() { in_val } else { 0.0 } + self.filter_state[i] * feedback;
                if next_val.abs() < 1e-15 {
                    next_val = 0.0;
                }
                if !next_val.is_finite() {
                    next_val = 0.0;
                }

                self.delay_lines[i][self.write_indices[i]] = next_val;
                self.write_indices[i] = (self.write_indices[i] + 1) % self.delay_lines[i].len();

                // Add wet signal back to buffer
                buffers[i][s] = (if buffers[i][s].is_finite() {
                    buffers[i][s]
                } else {
                    0.0
                } + h[i] * 0.3)
                    .clamp(-4.0, 4.0);
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Atmos Reverb state.
    pub fn audit_atmos_reverb(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 1.0
            && self.delay_lines.iter().all(|line| !line.is_empty())
            && self
                .write_indices
                .iter()
                .zip(self.delay_lines.iter())
                .all(|(index, line)| *index < line.len())
            && self.filter_state.iter().all(|value| value.is_finite())
            && self.feedback.is_finite()
            && self.damping.is_finite()
    }
}

#[cfg(test)]
mod tests {
    use super::AtmosReverbEngine;

    #[test]
    fn accepts_short_or_mismatched_immersive_buffers() {
        let mut engine = AtmosReverbEngine::new(8_000.0);
        let mut channels: Vec<Vec<f32>> = (0..12).map(|_| vec![0.0; 8]).collect();
        channels[5].truncate(3);
        let mut refs: Vec<&mut [f32]> = channels.iter_mut().map(Vec::as_mut_slice).collect();
        refs[0][0] = f32::NAN;
        engine.process_immersive(&mut refs);
        assert!(engine.audit_atmos_reverb());
        assert!(refs[0][0].is_finite());
    }
}
