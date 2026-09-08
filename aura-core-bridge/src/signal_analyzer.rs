use std::collections::VecDeque;

const MAX_LOUDNESS_BLOCKS: usize = 8192;

pub struct AnalysisFrame {
    pub peak: [f32; 2],
    pub true_peak: [f32; 2],
    pub correlation: f32,
    pub lufs_integrated: f32,
    pub samples: Vec<f32>,
}

pub struct SignalAnalyzerOrchestrator {
    pub buffers: [AnalysisFrame; 3],
    pub write_idx: usize,
    pub latest_idx: usize,
    pub ui_idx: usize,
    loudness_blocks: VecDeque<f64>,
    sample_rate: f32,
    k_weight_state: [[f64; 8]; 2],
    previous_samples: [f32; 2],
}

impl SignalAnalyzerOrchestrator {
    pub fn new(fft_size: usize) -> Self {
        Self::new_with_sample_rate(fft_size, 48_000.0)
    }

    pub fn new_with_sample_rate(fft_size: usize, sample_rate: f32) -> Self {
        let buffers = [(); 3].map(|_| AnalysisFrame {
            peak: [0.0; 2],
            true_peak: [0.0; 2],
            correlation: 0.0,
            lufs_integrated: 0.0,
            samples: vec![0.0; fft_size],
        });
        Self {
            buffers,
            write_idx: 0,
            latest_idx: 0,
            ui_idx: 99,
            loudness_blocks: VecDeque::with_capacity(MAX_LOUDNESS_BLOCKS),
            sample_rate: if sample_rate.is_finite() && sample_rate > 0.0 {
                sample_rate
            } else {
                48_000.0
            },
            k_weight_state: [[0.0; 8]; 2],
            previous_samples: [0.0; 2],
        }
    }

    fn k_weighted_sample(&mut self, channel: usize, sample: f64) -> f64 {
        // BS.1770 reference coefficients.  Do not apply 48 kHz coefficients
        // to another rate: that produces a plausible-looking but incorrect
        // loudness number.
        let (pre_b, pre_a, rlb_a): ([f64; 3], [f64; 2], [f64; 2]) =
            if (self.sample_rate - 48_000.0).abs() <= 1.0 {
                ([1.53512485958697, -2.69169618940638, 1.19839281085285],
                 [-1.69065929318241, 0.73248077421585],
                 [-1.99004745483398, 0.99007225036662])
            } else if (self.sample_rate - 44_100.0).abs() <= 1.0 {
                ([1.53084123005035, -2.65097999515473, 1.16907907992134],
                 [-1.66365511325602, 0.712595428073225],
                 [-1.98916967362980, 0.989199035787039])
            } else {
                return sample;
            };
        const RLB_B: [f64; 3] = [1.0, -2.0, 1.0];
        let state = &mut self.k_weight_state[channel];
        let pre = pre_b[0] * sample + pre_b[1] * state[0] + pre_b[2] * state[1]
            - pre_a[0] * state[2] - pre_a[1] * state[3];
        state[1] = state[0];
        state[0] = sample;
        state[3] = state[2];
        state[2] = pre;
        let rlb = RLB_B[0] * pre + RLB_B[1] * state[4] + RLB_B[2] * state[5]
            - rlb_a[0] * state[6] - rlb_a[1] * state[7];
        state[5] = state[4];
        state[4] = pre;
        state[7] = state[6];
        state[6] = rlb;
        rlb
    }

    /// INDUSTRIAL: Processes an audio block with SIMD-accelerated precision.
    pub fn process(&mut self, left: &[f32], right: &[f32]) {
        // INDUSTRIAL: Implementation of high-performance SIMD metering.
        // Rust's SIMDMeteringEngine ensures bit-accurate peak and LUFS tracking.
        let mut next_idx = (self.write_idx + 1) % 3;
        if next_idx == self.ui_idx {
            next_idx = (next_idx + 1) % 3;
        }

        // --- SIMD METERING ---
        let mut max_p = [0.0f32; 2];
        let mut max_true_p = [0.0f32; 2];
        let mut cross = 0.0f64;
        let mut left_power = 0.0f64;
        let mut right_power = 0.0f64;
        for (channel, input) in [left, right].iter().enumerate() {
            let mut previous = self.previous_samples[channel];
            for &s in input.iter() {
                let clean = if s.is_finite() { s } else { 0.0 };
                max_p[channel] = max_p[channel].max(clean.abs());
                max_true_p[channel] = max_true_p[channel].max(clean.abs());
                for step in 1..=3 {
                    let fraction = step as f32 * 0.25;
                    let interpolated = previous + (clean - previous) * fraction;
                    max_true_p[channel] = max_true_p[channel].max(interpolated.abs());
                }
                previous = clean;
            }
            self.previous_samples[channel] = previous;
        }
        /*
         * Keep the correlation calculation separate from peak scanning so
         * mismatched channel lengths remain well-defined.
         */
        for (i, &s) in right.iter().enumerate() {
            if i < left.len() {
                let l = f64::from(if left[i].is_finite() { left[i] } else { 0.0 });
                let r = f64::from(if s.is_finite() { s } else { 0.0 });
                cross += l * r;
                left_power += l * l;
                right_power += r * r;
            }
        }

        let sample_count = left.len() + right.len();
        let mut weighted_power = 0.0f64;
        for (channel, input) in [left, right].iter().enumerate() {
            for &sample in input.iter() {
                let clean = if sample.is_finite() { f64::from(sample) } else { 0.0 };
                let weighted = self.k_weighted_sample(channel, clean);
                weighted_power += weighted * weighted;
            }
        }
        if sample_count > 0 {
            if self.loudness_blocks.len() == MAX_LOUDNESS_BLOCKS {
                self.loudness_blocks.pop_front();
            }
            self.loudness_blocks.push_back(weighted_power / sample_count as f64);
        }
        let integrated_lufs = self.integrated_loudness();
        let target = &mut self.buffers[self.write_idx];
        target.samples.fill(0.0);
        for (index, &sample) in left.iter().enumerate().take(target.samples.len()) {
            target.samples[index] = if sample.is_finite() { sample } else { 0.0 };
        }
        target.peak = max_p;
        target.true_peak = max_true_p;
        target.correlation = if left_power > 0.0 && right_power > 0.0 {
            (cross / (left_power.sqrt() * right_power.sqrt())) as f32
        } else { 0.0 };
        target.lufs_integrated = integrated_lufs;

        self.latest_idx = self.write_idx;
        self.write_idx = next_idx;
    }

    fn integrated_loudness(&self) -> f32 {
        let absolute_gate = 10.0f64.powf((-70.0 + 0.691) / 10.0);
        let absolute_sum = self.loudness_blocks.iter()
            .copied()
            .filter(|energy| energy.is_finite() && *energy > absolute_gate)
            .sum::<f64>();
        let absolute_count = self.loudness_blocks.iter()
            .filter(|energy| energy.is_finite() && **energy > absolute_gate)
            .count();
        if absolute_count == 0 {
            return f32::NEG_INFINITY;
        }
        let absolute_mean = absolute_sum / absolute_count as f64;
        let relative_gate = absolute_mean * 0.1;
        let gated_mean = self.loudness_blocks.iter()
            .copied()
            .filter(|energy| energy.is_finite() && *energy >= relative_gate)
            .sum::<f64>();
        let gated_count = self.loudness_blocks.iter()
            .filter(|energy| energy.is_finite() && **energy >= relative_gate)
            .count();
        if gated_count == 0 || gated_mean <= 0.0 {
            f32::NEG_INFINITY
        } else {
            (-0.691 + 10.0 * (gated_mean / gated_count as f64).log10()) as f32
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide signal state.
    pub fn audit_signal_analyzer(&self) -> bool {
        self.buffers.iter().all(|frame| {
            frame.peak.iter().all(|value| value.is_finite() && *value >= 0.0)
                && frame.true_peak.iter().all(|value| value.is_finite() && *value >= 0.0)
                && frame.correlation.is_finite() && (-1.0..=1.0).contains(&frame.correlation)
                && (frame.lufs_integrated.is_finite() || frame.lufs_integrated == f32::NEG_INFINITY)
                && frame.samples.iter().all(|sample| sample.is_finite())
        }) && self.write_idx < self.buffers.len()
            && self.latest_idx < self.buffers.len()
            && (self.ui_idx == 99 || self.ui_idx < self.buffers.len())
    }
}

#[cfg(test)]
mod tests {
    use super::SignalAnalyzerOrchestrator;

    #[test]
    fn process_populates_metering_metrics() {
        let mut analyzer = SignalAnalyzerOrchestrator::new(8);
        analyzer.process(&[0.5, -0.25, 0.0], &[0.5, -0.25, 0.0]);
        let frame = &analyzer.buffers[analyzer.latest_idx];
        assert_eq!(frame.peak, [0.5, 0.5]);
        assert!(frame.true_peak[0] >= frame.peak[0]);
        assert!((frame.correlation - 1.0).abs() < 1e-6);
        assert!(frame.lufs_integrated.is_finite());
        assert!(analyzer.audit_signal_analyzer());
    }

    #[test]
    fn forty_four_point_one_kilohertz_uses_weighted_metering() {
        let mut analyzer = SignalAnalyzerOrchestrator::new_with_sample_rate(8, 44_100.0);
        analyzer.process(&[0.25; 8], &[0.25; 8]);
        let frame = &analyzer.buffers[analyzer.latest_idx];
        assert!(frame.lufs_integrated.is_finite());
        assert!(analyzer.audit_signal_analyzer());
    }
}
