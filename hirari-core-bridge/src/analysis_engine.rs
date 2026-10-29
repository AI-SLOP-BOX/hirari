use crate::k_weighting_filter::LegacyKWeightingFilter;
use crate::spectrum_analyzer::SpectrumState;
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

const ENERGY_HISTORY_SIZE: usize = 1000;

struct AnalysisCore {
    filter: LegacyKWeightingFilter,
    spectrum_left: SpectrumState,
    spectrum_right: SpectrumState,
    stats: [f32; 4],
    total_energy: f64,
    measurements_count: u64,
    energy_history: [f32; ENERGY_HISTORY_SIZE],
    energy_write: usize,
    energy_count: usize,
    energy_sum: f64,
}

impl AnalysisCore {
    fn new() -> Self {
        Self {
            // AnalysisEngine's C++ constructor leaves KWeightingFilter at its
            // default 44.1 kHz rate, independent of its spectrum sample rate.
            filter: LegacyKWeightingFilter::new(44_100.0),
            spectrum_left: SpectrumState::new(),
            spectrum_right: SpectrumState::new(),
            stats: [-70.0, -70.0, -70.0, -100.0],
            total_energy: 0.0,
            measurements_count: 0,
            energy_history: [0.0; ENERGY_HISTORY_SIZE],
            energy_write: 0,
            energy_count: 0,
            energy_sum: 0.0,
        }
    }

    fn update_loudness(&mut self, left: &[f32], right: &[f32], sample_rate: f64) {
        if left.is_empty()
            || left.len() != right.len()
            || !sample_rate.is_finite()
            || sample_rate <= 0.0
        {
            return;
        }

        self.spectrum_left.process(left, sample_rate);
        self.spectrum_right.process(right, sample_rate);

        let mut current_energy_sum = 0.0_f64;
        for (&left_sample, &right_sample) in left.iter().zip(right) {
            let (weighted_left, weighted_right) =
                self.filter.process_stereo(left_sample, right_sample);
            current_energy_sum += (weighted_left as f64) * (weighted_left as f64)
                + (weighted_right as f64) * (weighted_right as f64);
        }

        let denominator = left.len() as f64 * 2.0 + 1.0e-10;
        let mean_energy = (current_energy_sum / denominator) as f32;
        let momentary_lufs = -0.691_f32 + 10.0_f32 * (mean_energy + 1.0e-12_f32).log10();
        self.stats[0] = momentary_lufs;

        if momentary_lufs > -70.0 {
            let energy = if mean_energy.is_finite() {
                mean_energy.max(0.0)
            } else {
                0.0
            };
            if self.energy_count < ENERGY_HISTORY_SIZE {
                self.energy_history[self.energy_write] = energy;
                self.energy_sum += energy as f64;
                self.energy_count += 1;
            } else {
                self.energy_sum -= self.energy_history[self.energy_write] as f64;
                self.energy_history[self.energy_write] = energy;
                self.energy_sum += energy as f64;
            }
            self.energy_write = (self.energy_write + 1) % ENERGY_HISTORY_SIZE;

            let average_energy = self.energy_sum / self.energy_count.max(1) as f64;
            let relative_threshold =
                -0.691_f64 + 10.0 * (average_energy + 1.0e-12_f64).log10() - 10.0;
            if momentary_lufs as f64 > relative_threshold {
                self.total_energy += mean_energy as f64;
                self.measurements_count += 1;
                let average = self.total_energy / self.measurements_count as f64;
                self.stats[2] = (-0.691_f64 + 10.0 * (average + 1.0e-12_f64).log10()) as f32;
            }
        }

        for (&left_sample, &right_sample) in left.iter().zip(right) {
            let left_peak = left_sample.abs();
            let right_peak = right_sample.abs();
            // Match std::max(left_peak, right_peak), including its NaN ordering.
            let peak = if left_peak < right_peak {
                right_peak
            } else {
                left_peak
            };
            let db = 20.0_f32 * (peak + 1.0e-12_f32).log10();
            if db > self.stats[3] {
                self.stats[3] = db;
            }
        }
    }
}

struct AnalysisHandle {
    core: UnsafeCell<AnalysisCore>,
    stats: [AtomicU32; 4],
}

#[no_mangle]
pub extern "C" fn hirari_analysis_engine_create(_sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(AnalysisHandle {
        core: UnsafeCell::new(AnalysisCore::new()),
        stats: [
            AtomicU32::new((-70.0_f32).to_bits()),
            AtomicU32::new((-70.0_f32).to_bits()),
            AtomicU32::new((-70.0_f32).to_bits()),
            AtomicU32::new((-100.0_f32).to_bits()),
        ],
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_analysis_engine_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<AnalysisHandle>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_analysis_engine_update(
    state: *mut c_void,
    left: *const f32,
    right: *const f32,
    frames: u32,
    sample_rate: f64,
) {
    if state.is_null() || left.is_null() || right.is_null() || frames == 0 {
        return;
    }
    let handle = &*state.cast::<AnalysisHandle>();
    let left = std::slice::from_raw_parts(left, frames as usize);
    let right = std::slice::from_raw_parts(right, frames as usize);
    let core = &mut *handle.core.get();
    core.update_loudness(left, right, sample_rate);
    for (target, value) in handle.stats.iter().zip(core.stats) {
        target.store(value.to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_analysis_engine_get_stats(state: *const c_void, output: *mut f32) {
    if state.is_null() || output.is_null() {
        return;
    }
    let handle = &*state.cast::<AnalysisHandle>();
    for (index, value) in handle.stats.iter().enumerate() {
        *output.add(index) = f32::from_bits(value.load(Ordering::Relaxed));
    }
}

/// Per-track spectrogram features for `Track::getAnalysisEngine()`.
///
/// The first channel is split into `bins` equal sample ranges (the caller
/// passes `min(32, frames)`), each range reports `0.5 * (rms + peak)` clamped
/// to 0..1. Non-finite samples count as silence, and a range whose rms or peak
/// is non-finite reports 0. Writes at most `capacity` values and returns how
/// many were written.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_analysis_spectrogram(
    samples: *const f32,
    sample_count: u32,
    output: *mut f32,
    capacity: usize,
) -> usize {
    if samples.is_null() || output.is_null() || sample_count == 0 || capacity == 0 {
        return 0;
    }
    let source = std::slice::from_raw_parts(samples, sample_count as usize);
    let bins = (capacity as u32).min(sample_count);
    for bin in 0..bins {
        // Split with the same integer arithmetic the native loop used.
        let begin = ((bin * sample_count) / bins) as usize;
        let end = (((bin + 1) * sample_count) / bins) as usize;
        let count = if end > begin { end - begin } else { 1 };
        let mut energy = 0.0_f64;
        let mut peak = 0.0_f32;
        for &raw in &source[begin..end] {
            let sample = if raw.is_finite() { raw } else { 0.0 };
            energy += (sample as f64) * (sample as f64);
            peak = peak.max(sample.abs());
        }
        let rms = (energy / count as f64).sqrt() as f32;
        *output.add(bin as usize) = if rms.is_finite() && peak.is_finite() {
            (0.5_f32 * (rms + peak)).clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
    bins as usize
}

#[no_mangle]
pub unsafe extern "C" fn hirari_analysis_engine_get_band(
    state: *const c_void,
    channel: u32,
    band: u32,
) -> f32 {
    if state.is_null() || channel > 1 {
        return 0.0;
    }
    let core = &*(*state.cast::<AnalysisHandle>()).core.get();
    if channel == 0 {
        core.spectrum_left.get_band(band as usize)
    } else {
        core.spectrum_right.get_band(band as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_spectrogram_splits_and_clamps_ranges() {
        // 8 samples, 4 bins: each bin is 2 samples.
        let samples = [0.5_f32, -0.5, 2.0, -2.0, 0.0, 0.0, 1.0, 1.0];
        let mut out = [0.0_f32; 4];
        let written = unsafe {
            hirari_track_analysis_spectrogram(samples.as_ptr(), 8, out.as_mut_ptr(), out.len())
        };
        assert_eq!(written, 4);
        // bin0: rms=0.5, peak=0.5 -> 0.5
        assert!((out[0] - 0.5).abs() < 1e-6);
        // bin1: rms=2, peak=2 -> 2 clamped to 1
        assert_eq!(out[1], 1.0);
        // bin2: silence -> 0
        assert_eq!(out[2], 0.0);
        // bin3: rms=1, peak=1 -> 1
        assert_eq!(out[3], 1.0);
    }

    #[test]
    fn track_spectrogram_rejects_empty_input() {
        let mut out = [0.0_f32; 4];
        assert_eq!(unsafe {
            hirari_track_analysis_spectrogram(std::ptr::null(), 8, out.as_mut_ptr(), out.len())
        }, 0);
        assert_eq!(unsafe {
            hirari_track_analysis_spectrogram(out.as_ptr(), 0, out.as_mut_ptr(), out.len())
        }, 0);
    }

    #[test]
    fn track_spectrogram_treats_non_finite_as_silence() {
        let samples = [f32::NAN, f32::INFINITY];
        let mut out = [1.0_f32; 1];
        let written = unsafe {
            hirari_track_analysis_spectrogram(samples.as_ptr(), 2, out.as_mut_ptr(), out.len())
        };
        assert_eq!(written, 1);
        assert_eq!(out[0], 0.0);
    }
}
