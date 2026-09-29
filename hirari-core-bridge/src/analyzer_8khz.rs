use std::ffi::c_void;
use std::slice;
use std::sync::atomic::{AtomicU32, Ordering};

/// Stateful 8 kHz band-energy analyzer used by the realtime analysis path.
/// Filter state remains owned by one processing thread; published meters are
/// atomic because the UI may read them concurrently.
pub struct Analyzer8kHzState {
    sample_rate: f64,
    high_pass_state: f32,
    low_pass_state: f32,
    rms: AtomicU32,
    peak: AtomicU32,
    high_freq_energy: AtomicU32,
}

impl Analyzer8kHzState {
    fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate: valid_sample_rate(sample_rate),
            high_pass_state: 0.0,
            low_pass_state: 0.0,
            rms: AtomicU32::new(0.0f32.to_bits()),
            peak: AtomicU32::new(0.0f32.to_bits()),
            high_freq_energy: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    fn analyze(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            self.publish(0.0, 0.0, 0.0);
            return;
        }

        let high_pass_coeff =
            (1.0 / (1.0 + (2.0 * std::f64::consts::PI * 6000.0 / self.sample_rate))) as f32;
        let low_pass_coeff =
            (1.0 - (-(2.0 * std::f64::consts::PI * 10_000.0) / self.sample_rate).exp()) as f32;
        let mut sum_squares = 0.0f64;
        let mut band_sum_squares = 0.0f64;
        let mut peak = 0.0f32;

        for &sample in samples {
            if !sample.is_finite() {
                continue;
            }
            peak = peak.max(sample.abs());
            sum_squares += (sample as f64) * (sample as f64);

            self.high_pass_state += high_pass_coeff * (sample - self.high_pass_state);
            let high_passed = sample - self.high_pass_state;
            self.low_pass_state += low_pass_coeff * (high_passed - self.low_pass_state);
            band_sum_squares += (self.low_pass_state as f64) * (self.low_pass_state as f64);
        }

        let divisor = samples.len() as f64;
        let rms = (sum_squares / divisor).sqrt() as f32;
        let band_rms = (band_sum_squares / divisor).sqrt() as f32;
        self.publish(rms, peak, band_rms);
    }

    fn publish(&self, rms: f32, peak: f32, energy: f32) {
        self.rms
            .store(finite_or_zero(rms).to_bits(), Ordering::Relaxed);
        self.peak
            .store(finite_or_zero(peak).to_bits(), Ordering::Relaxed);
        self.high_freq_energy
            .store(finite_or_zero(energy).to_bits(), Ordering::Relaxed);
    }
}

fn valid_sample_rate(sample_rate: f64) -> f64 {
    if sample_rate.is_finite() && sample_rate > 0.0 {
        sample_rate
    } else {
        44_100.0
    }
}

fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

#[no_mangle]
pub extern "C" fn hirari_analyzer_8khz_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(Analyzer8kHzState::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_analyzer_8khz_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<Analyzer8kHzState>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_analyzer_8khz_set_sample_rate(
    state: *mut c_void,
    sample_rate: f64,
) {
    let Some(state) = state.cast::<Analyzer8kHzState>().as_mut() else {
        return;
    };
    state.sample_rate = valid_sample_rate(sample_rate);
    state.high_pass_state = 0.0;
    state.low_pass_state = 0.0;
}

#[no_mangle]
pub unsafe extern "C" fn hirari_analyzer_8khz_analyze(
    state: *mut c_void,
    samples: *const f32,
    sample_count: usize,
) {
    let Some(state) = state.cast::<Analyzer8kHzState>().as_mut() else {
        return;
    };
    if samples.is_null() {
        state.publish(0.0, 0.0, 0.0);
        return;
    }
    state.analyze(slice::from_raw_parts(samples, sample_count));
}

#[no_mangle]
pub unsafe extern "C" fn hirari_analyzer_8khz_get_energy(state: *const c_void) -> f32 {
    state
        .cast::<Analyzer8kHzState>()
        .as_ref()
        .map(|state| f32::from_bits(state.high_freq_energy.load(Ordering::Relaxed)))
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyzer_publishes_finite_meter_values_and_resets_on_empty_input() {
        let mut analyzer = Analyzer8kHzState::new(48_000.0);
        analyzer.analyze(&[0.0; 256]);
        assert_eq!(analyzer.high_freq_energy.load(Ordering::Relaxed), 0);

        analyzer.analyze(&[0.5, f32::NAN, -0.25, f32::INFINITY]);
        assert!(f32::from_bits(analyzer.rms.load(Ordering::Relaxed)).is_finite());
        assert_eq!(f32::from_bits(analyzer.peak.load(Ordering::Relaxed)), 0.5);
        assert!(f32::from_bits(analyzer.high_freq_energy.load(Ordering::Relaxed)).is_finite());

        analyzer.analyze(&[]);
        assert_eq!(analyzer.rms.load(Ordering::Relaxed), 0);
        assert_eq!(analyzer.peak.load(Ordering::Relaxed), 0);
        assert_eq!(analyzer.high_freq_energy.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn analyzer_filter_state_continues_across_blocks_and_resets_on_rate_change() {
        let signal = (0..512)
            .map(|index| (index as f32 * 0.17).sin())
            .collect::<Vec<_>>();
        let mut whole = Analyzer8kHzState::new(44_100.0);
        whole.analyze(&signal);

        let mut blocks = Analyzer8kHzState::new(44_100.0);
        blocks.analyze(&signal[..173]);
        blocks.analyze(&signal[173..]);
        assert!((whole.high_pass_state - blocks.high_pass_state).abs() < 1.0e-6);
        assert!((whole.low_pass_state - blocks.low_pass_state).abs() < 1.0e-6);

        blocks.high_pass_state = 0.5;
        blocks.low_pass_state = -0.25;
        unsafe { hirari_analyzer_8khz_set_sample_rate(&mut blocks as *mut _ as *mut c_void, 0.0) };
        assert_eq!(blocks.sample_rate, 44_100.0);
        assert_eq!(blocks.high_pass_state, 0.0);
        assert_eq!(blocks.low_pass_state, 0.0);
    }
}
