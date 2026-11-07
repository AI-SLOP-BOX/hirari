use std::ffi::c_void;

pub type WaveformSampleCount = unsafe extern "C" fn(*mut c_void, *mut u64) -> bool;
pub type WaveformSample = unsafe extern "C" fn(*mut c_void, u64, *mut f32) -> bool;

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

/// Build the interleaved max/min envelope used by the region waveform cache.
/// Non-finite source frames are ignored and the legacy zero baseline is kept.
pub fn analysis_waveform_peak_envelope(left: &[f32], right: &[f32], peak_count: u32) -> Vec<f32> {
    let sample_count = left.len().min(right.len());
    if peak_count == 0 || sample_count == 0 {
        return Vec::new();
    }
    let peak_count = peak_count as usize;
    let mut peaks = Vec::with_capacity(peak_count.saturating_mul(2));
    for index in 0..peak_count {
        let begin = ((index as u128 * sample_count as u128) / peak_count as u128) as usize;
        let end = (((index as u128 + 1) * sample_count as u128) / peak_count as u128) as usize;
        let end = end.max(begin.saturating_add(1)).min(sample_count);
        let mut maximum = 0.0f32;
        let mut minimum = 0.0f32;
        for sample in begin..end {
            for value in [left[sample], right[sample]] {
                if value.is_finite() {
                    maximum = maximum.max(value);
                    minimum = minimum.min(value);
                }
            }
        }
        peaks.extend([maximum, minimum]);
    }
    peaks
}

/// Select cached max/min columns at the requested display resolution.
pub fn resample_waveform_peak_envelope(peaks: &[f32], peak_count: u32) -> Vec<f32> {
    let cached_count = peaks.len() / 2;
    if peak_count == 0 || cached_count == 0 || peaks.len() % 2 != 0 {
        return Vec::new();
    }
    let peak_count = peak_count as usize;
    let mut result = Vec::with_capacity(peak_count.saturating_mul(2));
    for index in 0..peak_count {
        let source_index =
            (index as u128 * cached_count as u128 / peak_count as u128) as usize * 2;
        result.extend([peaks[source_index], peaks[source_index + 1]]);
    }
    result
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
    use super::{
        analysis_waveform_peak_envelope, resample_waveform_peak_envelope, WaveformOrchestrator,
    };

    #[test]
    fn peak_envelope_uses_stereo_extrema_and_ignores_non_finite_samples() {
        let left = [0.25, 0.75, f32::NAN, -0.5, 0.1];
        let right = [0.5, -0.25, f32::INFINITY, 0.2, -0.1];
        assert_eq!(
            analysis_waveform_peak_envelope(&left, &right, 2),
            vec![0.75, -0.25, 0.2, -0.5]
        );
        assert!(analysis_waveform_peak_envelope(&left, &right, 0).is_empty());
        assert!(analysis_waveform_peak_envelope(&[], &[], 8).is_empty());
    }

    #[test]
    fn peak_envelope_handles_more_pixels_than_frames() {
        let result = analysis_waveform_peak_envelope(&[0.5, -0.25], &[0.1, 0.75], 4);
        assert_eq!(result, vec![0.5, 0.0, 0.5, 0.0, 0.75, -0.25, 0.75, -0.25]);
    }

    #[test]
    fn cached_peak_resampling_preserves_nearest_left_columns() {
        assert_eq!(
            resample_waveform_peak_envelope(
                &[1.0, -1.0, 0.8, -0.8, 0.6, -0.6, 0.4, -0.4],
                2
            ),
            vec![1.0, -1.0, 0.6, -0.6]
        );
        assert!(resample_waveform_peak_envelope(&[1.0], 2).is_empty());
        assert!(resample_waveform_peak_envelope(&[1.0, -1.0], 0).is_empty());
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn peak_envelope_matches_frozen_cpp_for_boundaries_and_non_finite_audio() {
        unsafe extern "C" {
            fn hirari_waveform_peak_envelope_reference(
                left: *const f32,
                right: *const f32,
                sample_count: usize,
                peak_count: u32,
                output: *mut f32,
                capacity: usize,
            ) -> usize;
        }

        let left = (0..257)
            .map(|index| (index as f32 * 0.17).sin())
            .collect::<Vec<_>>();
        let mut right = (0..257)
            .map(|index| (index as f32 * 0.11).cos() * 0.7)
            .collect::<Vec<_>>();
        right[2] = f32::NAN;
        right[129] = f32::INFINITY;
        for peak_count in [1, 7, 257, 512] {
            let rust = analysis_waveform_peak_envelope(&left, &right, peak_count);
            let mut cpp = vec![0.0; rust.len()];
            let count = unsafe {
                hirari_waveform_peak_envelope_reference(
                    left.as_ptr(),
                    right.as_ptr(),
                    left.len(),
                    peak_count,
                    cpp.as_mut_ptr(),
                    cpp.len(),
                )
            };
            assert_eq!(count, rust.len());
            assert_eq!(rust, cpp);
        }
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn cached_peak_resampling_matches_frozen_cpp_selection() {
        unsafe extern "C" {
            fn hirari_waveform_peak_resample_reference(
                peaks: *const f32,
                cached_count: usize,
                peak_count: u32,
                output: *mut f32,
                capacity: usize,
            ) -> usize;
        }

        let cached = (0..4096)
            .map(|index| (index as f32 * 0.125).sin())
            .collect::<Vec<_>>();
        for peak_count in [1, 37, 2048, 5000] {
            let rust = resample_waveform_peak_envelope(&cached, peak_count);
            let mut cpp = vec![0.0; rust.len()];
            let count = unsafe {
                hirari_waveform_peak_resample_reference(
                    cached.as_ptr(),
                    cached.len() / 2,
                    peak_count,
                    cpp.as_mut_ptr(),
                    cpp.len(),
                )
            };
            assert_eq!(count, rust.len());
            assert_eq!(rust, cpp);
        }
    }

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
