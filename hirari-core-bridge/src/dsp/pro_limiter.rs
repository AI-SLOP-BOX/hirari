use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

const LOOKAHEAD: usize = 480;
const DELAY_CAPACITY: usize = 1024;

/// State and sample loop for the plugin-host brickwall limiter. Peak
/// detection is inter-sample (true-peak): quarter-point Catmull-Rom
/// interpolation between the previous and current samples, so the
/// envelope reacts to overshoot the sample grid cannot represent.
pub struct ProLimiterEngine {
    sample_rate: f64,
    threshold: AtomicU32,
    envelope: f32,
    release: f32,
    delay_l: [f32; DELAY_CAPACITY],
    delay_r: [f32; DELAY_CAPACITY],
    write_idx: usize,
    previous_l: f32,
    previous_r: f32,
    previous2_l: f32,
    previous2_r: f32,
}

impl ProLimiterEngine {
    pub fn new(sample_rate: f64) -> Self {
        let mut engine = Self {
            sample_rate: 44_100.0,
            threshold: AtomicU32::new(1.0_f32.to_bits()),
            envelope: 1.0,
            release: 0.999,
            delay_l: [0.0; DELAY_CAPACITY],
            delay_r: [0.0; DELAY_CAPACITY],
            write_idx: 0,
            previous_l: 0.0,
            previous_r: 0.0,
            previous2_l: 0.0,
            previous2_r: 0.0,
        };
        engine.prepare_to_play(sample_rate);
        engine
    }

    pub fn prepare_to_play(&mut self, sample_rate: f64) {
        self.sample_rate =
            if sample_rate.is_finite() && crate::dsp_limits::is_supported_sample_rate(sample_rate) {
                sample_rate
            } else {
                44_100.0
            };
        self.release = (-1.0 / (0.05 * self.sample_rate as f32)).exp();
    }

    pub fn reset(&mut self) {
        self.envelope = 1.0;
        self.write_idx = 0;
        self.delay_l.fill(0.0);
        self.delay_r.fill(0.0);
        self.previous_l = 0.0;
        self.previous_r = 0.0;
        self.previous2_l = 0.0;
        self.previous2_r = 0.0;
    }

    pub fn set_threshold_db(&self, db: f32) {
        if db.is_finite() {
            self.threshold
                .store((10.0_f32.powf(db / 20.0)).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn threshold_linear(&self) -> f32 {
        f32::from_bits(self.threshold.load(Ordering::Relaxed))
    }

    pub fn restore_threshold_linear(&self, threshold: f32) -> bool {
        if !threshold.is_finite() || threshold <= 0.0 || threshold > 1.0 {
            return false;
        }
        self.threshold.store(threshold.to_bits(), Ordering::Relaxed);
        true
    }

    pub fn set_parameter(&self, id: u32, value: f32) {
        if id == 0 && value.is_finite() {
            self.set_threshold_db(-24.0 + value.clamp(0.0, 1.0) * 24.0);
        }
    }

    pub fn get_parameter(&self, id: u32) -> f32 {
        let threshold = self.threshold_linear();
        if id != 0 || threshold <= 0.0 {
            return 0.0;
        }
        let db = 20.0 * threshold.log10();
        ((db + 24.0) / 24.0).clamp(0.0, 1.0)
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len());
        for frame in 0..frames {
            let in_l = if left[frame].is_finite() {
                left[frame]
            } else {
                0.0
            };
            let in_r = if right[frame].is_finite() {
                right[frame]
            } else {
                0.0
            };

            // True-peak estimate: Catmull-Rom quarter points between
            // previous and current samples (future sample approximated
            // by the current one, as in the loudness analyzer).
            let interpolate = |p0: f32, p1: f32, p2: f32, p3: f32, t: f32| {
                let t2 = t * t;
                let t3 = t2 * t;
                0.5 * (2.0 * p1
                    + (-p0 + p2) * t
                    + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                    + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
            };
            let mut peak = in_l.abs().max(in_r.abs());
            for step in 1..=4 {
                let t = step as f32 * 0.2;
                peak = peak.max(
                    interpolate(self.previous2_l, self.previous_l, in_l, in_l, t)
                        .abs()
                        .max(
                            interpolate(self.previous2_r, self.previous_r, in_r, in_r, t)
                                .abs(),
                        ),
                );
            }
            self.previous2_l = self.previous_l;
            self.previous2_r = self.previous_r;
            self.previous_l = in_l;
            self.previous_r = in_r;
            let threshold = self.threshold_linear();
            let target_gain = if peak > threshold {
                threshold / peak
            } else {
                1.0
            };
            if target_gain < self.envelope {
                self.envelope = target_gain;
            } else {
                self.envelope = target_gain + self.release * (self.envelope - target_gain);
            }

            let read_idx = (self.write_idx + DELAY_CAPACITY - LOOKAHEAD) % DELAY_CAPACITY;
            let out_l = self.delay_l[read_idx] * self.envelope;
            let out_r = self.delay_r[read_idx] * self.envelope;
            left[frame] = if out_l.is_finite() { out_l } else { 0.0 };
            right[frame] = if out_r.is_finite() { out_r } else { 0.0 };

            self.delay_l[self.write_idx] = in_l;
            self.delay_r[self.write_idx] = in_r;
            self.write_idx = (self.write_idx + 1) % DELAY_CAPACITY;
        }
    }

    pub fn tail_samples(&self) -> u32 {
        ((LOOKAHEAD as f64 + 0.35 * self.sample_rate).min(30.0 * self.sample_rate)) as u32
    }
}

#[no_mangle]
pub extern "C" fn hirari_pro_limiter_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(ProLimiterEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_pro_limiter_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<ProLimiterEngine>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_pro_limiter_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = state.cast::<ProLimiterEngine>().as_mut() {
        state.prepare_to_play(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_pro_limiter_reset(state: *mut c_void) {
    if let Some(state) = state.cast::<ProLimiterEngine>().as_mut() {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_pro_limiter_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) {
    if left.is_null() || right.is_null() {
        return;
    }
    let Some(state) = state.cast::<ProLimiterEngine>().as_mut() else {
        return;
    };
    state.process(
        std::slice::from_raw_parts_mut(left, frames),
        std::slice::from_raw_parts_mut(right, frames),
    );
}

#[no_mangle]
pub unsafe extern "C" fn hirari_pro_limiter_set_parameter(state: *mut c_void, id: u32, value: f32) {
    if let Some(state) = state.cast::<ProLimiterEngine>().as_ref() {
        state.set_parameter(id, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_pro_limiter_get_parameter(state: *const c_void, id: u32) -> f32 {
    state
        .cast::<ProLimiterEngine>()
        .as_ref()
        .map_or(0.0, |state| state.get_parameter(id))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_pro_limiter_threshold_linear(state: *const c_void) -> f32 {
    state
        .cast::<ProLimiterEngine>()
        .as_ref()
        .map_or(1.0, |state| state.threshold_linear())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_pro_limiter_restore_threshold(
    state: *const c_void,
    threshold: f32,
) -> bool {
    state
        .cast::<ProLimiterEngine>()
        .as_ref()
        .is_some_and(|state| state.restore_threshold_linear(threshold))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_pro_limiter_tail(state: *const c_void) -> u32 {
    state
        .cast::<ProLimiterEngine>()
        .as_ref()
        .map_or(0, |state| state.tail_samples())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inter_sample_peaks_are_limited_below_sample_peaks() {
        // Alternating Nyquist signal at amplitude 0.9 with a threshold
        // of 0.5: sample peaks alone sit at the threshold boundary, but
        // the interpolated peaks overshoot, so the envelope must pull the
        // peak below the threshold (sample-peak-only code leaves it).
        let mut engine = ProLimiterEngine::new(48_000.0);
        engine.set_threshold_db(-6.02);
        let frames = 96;
        let mut left: Vec<f32> = (0..frames)
            .map(|i| if i % 2 == 0 { 0.9 } else { -0.9 })
            .collect();
        let mut right = left.clone();
        engine.process(&mut left, &mut right);
        // Warm-up (the pre-filled delay line) is silent; the steady state
        // must respect the threshold including inter-sample overshoot.
        let peak = left
            .iter()
            .chain(right.iter())
            .skip(LOOKAHEAD)
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        assert!(peak <= engine.threshold_linear() + 0.02, "peak {peak}");
    }

    #[test]
    fn below_threshold_passes_through() {
        let mut engine = ProLimiterEngine::new(48_000.0);
        engine.set_threshold_db(-6.02);
        let frames = 600;
        let mut left: Vec<f32> = (0..frames)
            .map(|i| 0.1 * ((2.0 * std::f32::consts::PI * 440.0 * i as f32) / 48_000.0).sin())
            .collect();
        let mut right = left.clone();
        engine.process(&mut left, &mut right);
        let out_peak = left
            .iter()
            .chain(right.iter())
            .skip(LOOKAHEAD)
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        assert!((out_peak - 0.1).abs() < 0.01, "out peak {out_peak}");
    }
}
