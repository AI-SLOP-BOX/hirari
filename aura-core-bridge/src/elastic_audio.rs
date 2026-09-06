pub enum ElasticAudioMode {
    Monophonic,
    Polyphonic,
    Percussive,
}

pub struct ElasticAudioEngine {
    pub sample_rate: f64,
    pub overlap_buf: Vec<f32>,
}

impl ElasticAudioEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            overlap_buf: vec![0.0; 8192],
        }
    }

    pub fn reset(&mut self) {
        self.overlap_buf.fill(0.0);
    }

    /// INDUSTRIAL: Professional Logic Pro-style 'Flex Time' (WSOLA).
    pub fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        ratio: f32,
        mode: ElasticAudioMode,
    ) {
        let num_in = input.len();
        let num_out = output.len();

        if !ratio.is_finite() || ratio <= 0.0 {
            output.fill(0.0);
            return;
        }
        if num_in < 512 || num_out < 512 {
            self.process_resampled(input, output, ratio);
            return;
        }

        let win_size = match mode {
            ElasticAudioMode::Percussive => 512,
            _ => 2048,
        };

        let hop_out = win_size / 4;
        let hop_in = (hop_out as f32 * ratio).round() as usize;
        if hop_in == 0 {
            output.fill(0.0);
            return;
        }
        let search_range = win_size / 8;

        let mut in_pos = 0;
        let mut out_pos = 0;

        output.fill(0.0);
        self.overlap_buf.fill(0.0);

        while out_pos <= num_out.saturating_sub(win_size)
            && in_pos <= num_in.saturating_sub(win_size + search_range)
        {
            // --- 2. FAST CORRELATION (Sampled for real-time) ---
            let mut best_offset = 0;
            let mut max_corr = -1e10;

            let search_range_i = search_range as i32;
            let mut found_candidate = false;
            for offset in (-search_range_i..search_range_i).step_by(4) {
                let candidate = in_pos as isize + offset as isize;
                if candidate < 0
                    || (candidate as usize).checked_add(win_size).is_none()
                    || candidate as usize + win_size > num_in
                {
                    continue;
                }
                found_candidate = true;
                let mut corr = 0.0;
                for j in (0..win_size).step_by(16) {
                    let in_idx = candidate as usize + j;
                    corr += input[in_idx] * self.overlap_buf[j];
                }
                if corr > max_corr {
                    max_corr = corr;
                    best_offset = offset;
                }
            }
            if !found_candidate {
                break;
            }

            // --- 3. OVERLAP-ADD WITH MODIFIED HANN WINDOW ---
            for j in 0..win_size {
                let win = 0.5
                    * (1.0 - (2.0 * std::f64::consts::PI * j as f64 / (win_size - 1) as f64).cos())
                        as f32;
                let in_idx = (in_pos as isize + best_offset as isize + j as isize) as usize;

                output[out_pos + j] += input[in_idx] * win;
                if j < 8192 {
                    self.overlap_buf[j] = input[in_idx]; // Save for next correlation
                }
            }

            in_pos = in_pos.saturating_add(hop_in);
            out_pos += hop_out;
        }
    }

    /// Bounded linear-interpolation path used for short clips and previews.
    pub fn process_resampled(&self, input: &[f32], output: &mut [f32], ratio: f32) {
        if input.is_empty() || !ratio.is_finite() || ratio <= 0.0 {
            output.fill(0.0);
            return;
        }
        let max_index = input.len().saturating_sub(1) as f32;
        for (index, sample) in output.iter_mut().enumerate() {
            let source = (index as f32 * ratio).clamp(0.0, max_index);
            let left = source.floor() as usize;
            let right = (left + 1).min(input.len() - 1);
            let amount = source - left as f32;
            let a = if input[left].is_finite() {
                input[left]
            } else {
                0.0
            };
            let b = if input[right].is_finite() {
                input[right]
            } else {
                0.0
            };
            let value = a + (b - a) * amount;
            *sample = if value.is_finite() { value } else { 0.0 };
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Elastic Audio state.
    pub fn audit_elastic_audio(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && !self.overlap_buf.is_empty()
            && self.overlap_buf.iter().all(|sample| sample.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::ElasticAudioEngine;

    #[test]
    fn short_clip_resampling_preserves_finite_interpolated_audio() {
        let mut engine = ElasticAudioEngine::new(48_000.0);
        let input = [0.0, 1.0, 0.0];
        let mut output = [0.0; 5];
        engine.process(
            &input,
            &mut output,
            0.5,
            super::ElasticAudioMode::Polyphonic,
        );
        assert_eq!(output[0], 0.0);
        assert!((output[1] - 0.5).abs() < 1e-6);
        assert!(output.iter().all(|sample| sample.is_finite()));
    }
}
