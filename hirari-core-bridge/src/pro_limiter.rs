use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

const LOOKAHEAD: usize = 480;
const DELAY_CAPACITY: usize = 1024;

/// State and sample loop for the plugin-host brickwall limiter.
pub struct ProLimiterEngine {
    sample_rate: f64,
    threshold: AtomicU32,
    envelope: f32,
    release: f32,
    delay_l: [f32; DELAY_CAPACITY],
    delay_r: [f32; DELAY_CAPACITY],
    write_idx: usize,
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
        };
        engine.prepare_to_play(sample_rate);
        engine
    }

    pub fn prepare_to_play(&mut self, sample_rate: f64) {
        self.sample_rate =
            if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
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

            let peak = in_l.abs().max(in_r.abs());
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
pub unsafe extern "C" fn hirari_pro_limiter_set_threshold_db(state: *const c_void, db: f32) {
    if let Some(state) = state.cast::<ProLimiterEngine>().as_ref() {
        state.set_threshold_db(db);
    }
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
