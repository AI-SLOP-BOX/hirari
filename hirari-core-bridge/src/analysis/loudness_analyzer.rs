use crate::k_weighting_filter::KWeightingFilterEngine;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    pub momentary_lufs: f32,
    pub short_term_lufs: f32,
    pub true_peak_db_l: f32,
    pub true_peak_db_r: f32,
    pub true_peak_db: f32,
}

pub struct LoudnessAnalyzerEngine {
    pub sample_rate: f64,
    pub filter: KWeightingFilterEngine,
    pub energy_buffer: Vec<f32>,
    pub momentary_sum: f32,
    pub short_term_sum: f32,
    pub write_idx: usize,
    pub momentary_window_size: usize,
    pub short_term_window_size: usize,
    pub latest_metrics: Metrics,
    momentary_sum_f64: f64,
    short_term_sum_f64: f64,
    previous_l: f32,
    previous_r: f32,
    previous2_l: f32,
    previous2_r: f32,
    /// Rolling per-sample energies for the gated integrated loudness:
    /// 400ms blocks at 75% overlap (100ms hop), as block mean energies.
    gated_window: Vec<f32>,
    gated_sum: f64,
    gated_count: usize,
    gated_hop: usize,
    gated_blocks: Vec<f64>,
}

fn safe_sample_rate(sr: f64) -> f64 {
    if sr.is_finite() && sr > 1.0 {
        sr
    } else {
        48_000.0
    }
}

fn window_size(seconds: f64, sr: f64) -> usize {
    (seconds * safe_sample_rate(sr)).round().max(1.0) as usize
}

impl LoudnessAnalyzerEngine {
    pub fn new(sample_rate: f64) -> Self {
        let sample_rate = safe_sample_rate(sample_rate);
        let momentary_window_size = window_size(0.4, sample_rate);
        let short_term_window_size = window_size(3.0, sample_rate);
        let energy_buffer = vec![0.0; short_term_window_size.next_power_of_two()];

        Self {
            sample_rate,
            filter: KWeightingFilterEngine::new(sample_rate),
            energy_buffer,
            momentary_sum: 0.0,
            short_term_sum: 0.0,
            write_idx: 0,
            momentary_window_size,
            short_term_window_size,
            latest_metrics: Metrics {
                momentary_lufs: -70.0,
                short_term_lufs: -70.0,
                true_peak_db_l: -100.0,
                true_peak_db_r: -100.0,
                true_peak_db: -100.0,
            },
            momentary_sum_f64: 0.0,
            short_term_sum_f64: 0.0,
            previous_l: 0.0,
            previous_r: 0.0,
            previous2_l: 0.0,
            previous2_r: 0.0,
            gated_window: vec![0.0; momentary_window_size],
            gated_sum: 0.0,
            gated_count: 0,
            gated_hop: 0,
            gated_blocks: Vec::new(),
        }
    }

    pub fn prepare_to_play(&mut self, sr: f64) {
        self.sample_rate = safe_sample_rate(sr);
        self.filter.set_sample_rate(self.sample_rate);
        self.momentary_window_size = window_size(0.4, self.sample_rate);
        self.short_term_window_size = window_size(3.0, self.sample_rate);
        self.energy_buffer = vec![0.0; self.short_term_window_size.next_power_of_two()];
        self.energy_buffer.fill(0.0);
        self.momentary_sum = 0.0;
        self.short_term_sum = 0.0;
        self.momentary_sum_f64 = 0.0;
        self.short_term_sum_f64 = 0.0;
        self.write_idx = 0;
        self.previous_l = 0.0;
        self.previous_r = 0.0;
        self.previous2_l = 0.0;
        self.previous2_r = 0.0;
        self.gated_window = vec![0.0; self.momentary_window_size];
        self.gated_sum = 0.0;
        self.gated_count = 0;
        self.gated_hop = 0;
        self.gated_blocks.clear();
    }

    fn gated_block_energy(loudness_lufs: f64) -> f64 {
        10f64.powf((loudness_lufs + 0.691) / 10.0)
    }

    /// BS.1770 gated integrated loudness over the accumulated 400ms
    /// blocks: absolute gate at −70 LUFS, then a relative gate 10 LU
    /// below the ungated mean of the survivors. Silence yields −70.
    pub fn integrated_lufs(&self) -> f32 {
        self.gated_loudness(-70.0, -10.0) as f32
    }

    /// Loudness range (EBU R128): 95th minus 10th percentile of the
    /// 400ms short-term loudness distribution after the absolute gate,
    /// with the relative gate 20 LU below the integrated value.
    pub fn loudness_range_lu(&self) -> f32 {
        let integrated = self.gated_loudness(-70.0, -10.0);
        if !integrated.is_finite() || integrated <= -69.9 {
            return 0.0;
        }
        let threshold = (integrated - 20.0).max(-70.0);
        let mut short_term: Vec<f64> = self
            .gated_blocks
            .iter()
            .map(|energy| -0.691 + 10.0 * energy.max(1e-12).log10())
            .filter(|loudness| *loudness >= threshold)
            .collect();
        if short_term.len() < 2 {
            return 0.0;
        }
        short_term.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let percentile = |p: f64| {
            let index = (p * (short_term.len() - 1) as f64).round() as usize;
            short_term[index.min(short_term.len() - 1)]
        };
        (percentile(0.95) - percentile(0.10)) as f32
    }

    fn gated_loudness(&self, absolute_gate: f64, relative_gate_lu: f64) -> f64 {
        let blocks: Vec<f64> = self
            .gated_blocks
            .iter()
            .copied()
            .filter(|energy| {
                -0.691 + 10.0 * energy.max(1e-12).log10() >= absolute_gate
            })
            .collect();
        if blocks.is_empty() {
            return absolute_gate;
        }
        let ungated_mean = blocks.iter().sum::<f64>() / blocks.len() as f64;
        let threshold_energy = Self::gated_block_energy(
            -0.691 + 10.0 * ungated_mean.max(1e-12).log10() + relative_gate_lu,
        );
        let survivors: Vec<f64> = blocks
            .into_iter()
            .filter(|energy| *energy >= threshold_energy)
            .collect();
        if survivors.is_empty() {
            return absolute_gate;
        }
        let mean = survivors.iter().sum::<f64>() / survivors.len() as f64;
        -0.691 + 10.0 * mean.max(1e-12).log10()
    }

    /// INDUSTRIAL: Processes an audio block with sliding windows and true peak estimation.
    pub fn process(&mut self, l: &[f32], r: &[f32]) -> Metrics {
        let num_frames = l.len().min(r.len());
        if num_frames == 0 {
            return self.latest_metrics;
        }
        let mask = self.energy_buffer.len() - 1;
        let mut max_l = 0.0f32;
        let mut max_r = 0.0f32;

        for i in 0..num_frames {
            let in_l = if l[i].is_finite() { l[i] } else { 0.0 };
            let in_r = if r[i].is_finite() { r[i] } else { 0.0 };
            let (out_l, out_r) = self.filter.process(in_l, in_r);
            let energy = ((out_l as f64 * out_l as f64 + out_r as f64 * out_r as f64) * 0.5)
                .min(f32::MAX as f64) as f32;

            // Sliding window updates
            let old_mom_idx = (self.write_idx.wrapping_sub(self.momentary_window_size)) & mask;
            let old_st_idx = (self.write_idx.wrapping_sub(self.short_term_window_size)) & mask;

            self.momentary_sum_f64 += energy as f64 - self.energy_buffer[old_mom_idx] as f64;
            self.short_term_sum_f64 += energy as f64 - self.energy_buffer[old_st_idx] as f64;
            self.momentary_sum = self.momentary_sum_f64.max(0.0).min(f32::MAX as f64) as f32;
            self.short_term_sum = self.short_term_sum_f64.max(0.0).min(f32::MAX as f64) as f32;

            self.energy_buffer[self.write_idx & mask] = energy;
            self.write_idx = self.write_idx.wrapping_add(1);

            // Gated integrated-loudness accumulation: rolling 400ms mean
            // energy, published every 100ms hop (75% overlap, BS.1770).
            let window_len = self.gated_window.len();
            if window_len > 0 {
                let slot = self.gated_count % window_len;
                self.gated_sum += energy as f64 - self.gated_window[slot] as f64;
                self.gated_window[slot] = energy;
                self.gated_count += 1;
                self.gated_hop += 1;
                if self.gated_hop >= (window_len / 4).max(1) {
                    if self.gated_count >= window_len {
                        self.gated_blocks
                            .push(self.gated_sum.max(0.0) / window_len as f64);
                    }
                    self.gated_hop = 0;
                }
            }

            // 4x Catmull–Rom inter-sample peak estimate. Previous samples
            // persist across callback blocks so a boundary transition is not missed.
            let tp_l = in_l.abs();
            let tp_r = in_r.abs();
            max_l = max_l.max(tp_l);
            max_r = max_r.max(tp_r);
            if i > 0 || self.write_idx > 1 {
                for step in 1..=4 {
                    let t = step as f32 * 0.2;
                    let t2 = t * t;
                    let t3 = t2 * t;
                    let interpolate = |p0: f32, p1: f32, p2: f32, p3: f32| {
                        0.5 * (2.0 * p1
                            + (-p0 + p2) * t
                            + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                            + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
                    };
                    max_l =
                        max_l.max(interpolate(self.previous2_l, self.previous_l, in_l, in_l).abs());
                    max_r =
                        max_r.max(interpolate(self.previous2_r, self.previous_r, in_r, in_r).abs());
                }
            }
            self.previous2_l = self.previous_l;
            self.previous2_r = self.previous_r;
            self.previous_l = in_l;
            self.previous_r = in_r;
        }

        let fast_log10 = |x: f32| (x + 1e-12).log10();

        let mut m = Metrics {
            momentary_lufs: -0.691
                + 10.0
                    * (self.momentary_sum_f64.max(0.0) / self.momentary_window_size as f64)
                        .max(1e-12)
                        .log10() as f32,
            short_term_lufs: -0.691
                + 10.0
                    * (self.short_term_sum_f64.max(0.0) / self.short_term_window_size as f64)
                        .max(1e-12)
                        .log10() as f32,
            true_peak_db_l: 20.0 * fast_log10(max_l),
            true_peak_db_r: 20.0 * fast_log10(max_r),
            true_peak_db: 0.0,
        };
        m.true_peak_db = m.true_peak_db_l.max(m.true_peak_db_r);

        self.latest_metrics = m;
        self.latest_metrics
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Loudness Analyzer state.
    pub fn audit_loudness_analyzer(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 1.0
            && !self.energy_buffer.is_empty()
            && self.energy_buffer.len().is_power_of_two()
            && self.write_idx < usize::MAX
            && self.momentary_window_size > 0
            && self.short_term_window_size >= self.momentary_window_size
            && self.short_term_window_size <= self.energy_buffer.len()
            && self.momentary_sum_f64.is_finite()
            && self.short_term_sum_f64.is_finite()
            && self.latest_metrics.momentary_lufs.is_finite()
            && self.latest_metrics.short_term_lufs.is_finite()
            && self.previous_l.is_finite()
            && self.previous_r.is_finite()
            && self.previous2_l.is_finite()
            && self.previous2_r.is_finite()
            && self.gated_window.len() == self.momentary_window_size
            && self.gated_sum.is_finite()
            && self.gated_blocks.iter().all(|energy| energy.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_block(analyzer: &mut LoudnessAnalyzerEngine, amplitude: f32, milliseconds: u64) {
        let frames = (analyzer.sample_rate * milliseconds as f64 / 1000.0) as usize;
        let l: Vec<f32> = (0..frames)
            .map(|i| {
                amplitude * ((2.0 * std::f32::consts::PI * 1000.0 * i as f32) / 48_000.0).sin()
            })
            .collect();
        let r = l.clone();
        analyzer.process(&l, &r);
    }

    #[test]
    fn silence_gates_to_floor() {
        let mut analyzer = LoudnessAnalyzerEngine::new(48_000.0);
        sine_block(&mut analyzer, 0.0, 2_000);
        assert_eq!(analyzer.integrated_lufs(), -70.0);
        assert_eq!(analyzer.loudness_range_lu(), 0.0);
    }

    #[test]
    fn tone_integrates_near_theoretical_level() {
        // 1kHz sine at 0.5 amplitude: unweighted mean square is 0.125 per
        // channel (-9.72 LUFS); the K pre-filter's high shelf adds gain at
        // 1kHz, so the result lands a few dB above that (bounded here).
        let mut analyzer = LoudnessAnalyzerEngine::new(48_000.0);
        sine_block(&mut analyzer, 0.5, 2_000);
        let integrated = analyzer.integrated_lufs();
        assert!((-9.72..-2.0).contains(&integrated), "got {integrated}");
        // One steady level has near-zero range.
        assert!(analyzer.loudness_range_lu() < 1.0);
        // Monotonicity: a louder tone integrates louder.
        let mut quieter = LoudnessAnalyzerEngine::new(48_000.0);
        sine_block(&mut quieter, 0.1, 2_000);
        assert!(quieter.integrated_lufs() < integrated - 10.0);
    }

    #[test]
    fn loudness_range_spans_level_changes() {
        // Loud half then quiet half: the 95th-10th percentile span of the
        // short-term distribution must be wide.
        let mut analyzer = LoudnessAnalyzerEngine::new(48_000.0);
        sine_block(&mut analyzer, 0.5, 1_000);
        sine_block(&mut analyzer, 0.05, 1_000);
        assert!(analyzer.loudness_range_lu() > 10.0);
        // The absolute gate keeps the quiet half included but the result
        // stays near the loud half's level.
        let integrated = analyzer.integrated_lufs();
        assert!(integrated > -16.0, "got {integrated}");
    }
}
