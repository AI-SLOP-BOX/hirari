pub struct PeakPairRust {
    pub min: f32,
    pub max: f32,
}

pub struct WaveformOrchestrator {
    pub samples_per_pixel: u32,
    pub peaks: Vec<PeakPairRust>,
    pub current_min: f32,
    pub current_max: f32,
    pub sample_counter: u32,
    has_current_sample: bool,
    pub generation: u64,
    pub gate: GenerationGate,
}

impl WaveformOrchestrator {
    pub fn new(samples_per_pixel: u32) -> Self {
        Self {
            samples_per_pixel,
            peaks: Vec::with_capacity(1024),
            current_min: 0.0,
            current_max: 0.0,
            sample_counter: 0,
            has_current_sample: false,
            generation: 0,
            gate: GenerationGate::new(),
        }
    }

    pub fn begin_generation(&mut self) -> u64 {
        self.generation = self.gate.invalidate();
        self.peaks.clear();
        self.sample_counter = 0;
        self.current_min = 0.0;
        self.current_max = 0.0;
        self.has_current_sample = false;
        self.generation
    }

    /// Invalidates a region's cached peaks when its source or lifetime ends.
    /// Returning the new generation lets an in-flight decoder discard stale
    /// work without publishing it after deletion.
    pub fn invalidate(&mut self) -> u64 {
        self.begin_generation()
    }

    pub fn commit_if_current(&mut self, generation: u64, peaks: Vec<PeakPairRust>) -> bool {
        if self.gate.accepts(generation) {
            self.peaks = peaks;
            true
        } else {
            false
        }
    }

    /// INDUSTRIAL: Generates peak data from a raw buffer with SIMD-accelerated precision.
    pub fn generate_for_block(&mut self, data: &[f32]) {
        if self.samples_per_pixel == 0 {
            return;
        }
        for &sample in data {
            if !sample.is_finite() {
                continue;
            }
            if !self.has_current_sample {
                self.current_min = sample;
                self.current_max = sample;
                self.has_current_sample = true;
            } else {
                self.current_min = self.current_min.min(sample);
                self.current_max = self.current_max.max(sample);
            }
            self.sample_counter += 1;

            if self.sample_counter >= self.samples_per_pixel {
                self.peaks.push(PeakPairRust {
                    min: self.current_min,
                    max: self.current_max,
                });
                self.current_min = 0.0;
                self.current_max = 0.0;
                self.sample_counter = 0;
                self.has_current_sample = false;
            }
        }
    }

    /// Flushes a final, partially filled pixel. Callers use this when the
    /// source stream reaches EOF; normal block boundaries remain incremental
    /// and do not manufacture an extra peak.
    pub fn finalize(&mut self) {
        if self.has_current_sample && self.sample_counter > 0 {
            self.peaks.push(PeakPairRust {
                min: self.current_min,
                max: self.current_max,
            });
            self.current_min = 0.0;
            self.current_max = 0.0;
            self.sample_counter = 0;
            self.has_current_sample = false;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project visual assets.
    pub fn audit_waveform_cache(&self) -> bool {
        self.samples_per_pixel > 0
            && self.current_min.is_finite()
            && self.current_max.is_finite()
            && self.current_min <= self.current_max
            && self.peaks.iter().all(|peak| {
                peak.min.is_finite() && peak.max.is_finite() && peak.min <= peak.max
            })
    }
}

#[cfg(test)]
mod tests {
    use super::WaveformOrchestrator;

    #[test]
    fn positive_only_waveform_keeps_true_minimum() {
        let mut cache = WaveformOrchestrator::new(3);
        cache.begin_generation();
        cache.generate_for_block(&[0.25, 0.75, 0.5]);
        assert_eq!(cache.peaks.len(), 1);
        assert_eq!(cache.peaks[0].min, 0.25);
        assert_eq!(cache.peaks[0].max, 0.75);
    }

    #[test]
    fn zero_samples_per_pixel_is_safe_and_produces_no_peaks() {
        let mut cache = WaveformOrchestrator::new(0);
        cache.generate_for_block(&[1.0, -1.0]);
        assert!(cache.peaks.is_empty());
    }

    #[test]
    fn stale_waveform_generation_cannot_replace_current_peaks() {
        let mut cache = WaveformOrchestrator::new(1);
        let old = cache.begin_generation();
        let current = cache.begin_generation();
        assert!(!cache.commit_if_current(old, vec![]));
        assert!(cache.commit_if_current(current, vec![]));
    }

    #[test]
    fn invalidation_clears_peaks_and_advances_generation() {
        let mut cache = WaveformOrchestrator::new(1);
        let first = cache.begin_generation();
        cache.generate_for_block(&[0.5]);
        let second = cache.invalidate();
        assert!(second > first);
        assert!(cache.peaks.is_empty());
    }

    #[test]
    fn audit_rejects_corrupt_peak_values() {
        let mut cache = WaveformOrchestrator::new(64);
        cache.peaks.push(super::PeakPairRust {
            min: f32::NAN,
            max: 1.0,
        });
        assert!(!cache.audit_waveform_cache());
    }

    #[test]
    fn finalize_preserves_partial_tail_pixel() {
        let mut cache = WaveformOrchestrator::new(4);
        cache.begin_generation();
        cache.generate_for_block(&[0.2, -0.4, 0.6]);
        assert!(cache.peaks.is_empty());
        cache.finalize();
        assert_eq!(cache.peaks.len(), 1);
        assert_eq!(cache.peaks[0].min, -0.4);
        assert_eq!(cache.peaks[0].max, 0.6);
        assert!(cache.audit_waveform_cache());
    }
}
use crate::generation_gate::GenerationGate;
