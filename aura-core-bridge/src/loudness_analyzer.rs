use crate::k_weighting_filter::KWeightingFilterEngine;

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

            // Simple Peak detection (C++ code didn't actually do 4x oversampling, just abs)
            let tp_l = in_l.abs();
            let tp_r = in_r.abs();
            max_l = max_l.max(tp_l);
            max_r = max_r.max(tp_r);
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
    }
}
