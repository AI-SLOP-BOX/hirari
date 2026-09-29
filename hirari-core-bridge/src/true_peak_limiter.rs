use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

/// Stateful stereo limiter. Its delay workspaces are allocated during prepare,
/// then reused without allocation by the audio callback.
pub struct TruePeakLimiterEngine {
    pub sample_rate: f64,
    pub delay_samples: u32,
    pub delay_l: Vec<f32>,
    pub delay_r: Vec<f32>,
    head: usize,
    pub current_gain: f32,
    pub z1_l: f32,
    pub z1_r: f32,
}

struct TruePeakLimiterState {
    engine: UnsafeCell<TruePeakLimiterEngine>,
    threshold_db: AtomicU32,
    ceiling_db: AtomicU32,
    latency: AtomicU32,
    tail: AtomicU32,
}

// The audio callback exclusively mutates the engine. UI parameter access is
// limited to the independent atomics; prepare/reset must be serialized by the host.
unsafe impl Sync for TruePeakLimiterState {}

impl TruePeakLimiterState {
    fn new(sample_rate: f64) -> Self {
        let engine = TruePeakLimiterEngine::new(sample_rate);
        let latency = engine.delay_samples;
        let tail = engine.get_tail_samples();
        Self {
            engine: UnsafeCell::new(engine),
            threshold_db: AtomicU32::new((-0.1f32).to_bits()),
            ceiling_db: AtomicU32::new((-0.1f32).to_bits()),
            latency: AtomicU32::new(latency),
            tail: AtomicU32::new(tail),
        }
    }

    fn set_parameter(&self, id: u32, value: f32) {
        if id > 1 || !value.is_finite() {
            return;
        }
        let db = -60.0 + value.clamp(0.0, 1.0) * 60.0;
        let destination = if id == 0 {
            &self.threshold_db
        } else {
            &self.ceiling_db
        };
        destination.store(db.to_bits(), Ordering::Relaxed);
    }

    fn parameter(&self, id: u32) -> f32 {
        let db = match id {
            0 => f32::from_bits(self.threshold_db.load(Ordering::Relaxed)),
            1 => f32::from_bits(self.ceiling_db.load(Ordering::Relaxed)),
            _ => return 0.0,
        };
        ((db + 60.0) / 60.0).clamp(0.0, 1.0)
    }

    fn prepare(&self, sample_rate: f64) {
        // SAFETY: the host serializes prepare against process and reset.
        let engine = unsafe { &mut *self.engine.get() };
        engine.prepare_to_play(sample_rate);
        self.latency.store(engine.delay_samples, Ordering::Relaxed);
        self.tail
            .store(engine.get_tail_samples(), Ordering::Relaxed);
    }

    fn reset(&self) {
        // SAFETY: the host serializes reset against process and prepare.
        unsafe { &mut *self.engine.get() }.reset();
    }

    fn process(&self, left: &mut [f32], right: &mut [f32]) {
        let threshold = f32::from_bits(self.threshold_db.load(Ordering::Relaxed));
        let ceiling = f32::from_bits(self.ceiling_db.load(Ordering::Relaxed));
        // SAFETY: the host calls the audio callback from one processing thread.
        unsafe { &mut *self.engine.get() }.process(left, right, threshold, ceiling);
    }
}

impl TruePeakLimiterEngine {
    pub fn new(sample_rate: f64) -> Self {
        let mut limiter = Self {
            sample_rate: 44_100.0,
            delay_samples: 0,
            delay_l: Vec::new(),
            delay_r: Vec::new(),
            head: 0,
            current_gain: 1.0,
            z1_l: 0.0,
            z1_r: 0.0,
        };
        limiter.prepare_to_play(sample_rate);
        limiter
    }

    pub fn prepare_to_play(&mut self, sample_rate: f64) {
        self.sample_rate =
            if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
                sample_rate
            } else {
                44_100.0
            };
        self.delay_samples = (self.sample_rate * 0.0015) as u32;
        let capacity = self.delay_samples as usize + 1;
        self.delay_l.resize(capacity, 0.0);
        self.delay_r.resize(capacity, 0.0);
        self.reset();
    }

    pub fn reset(&mut self) {
        self.current_gain = 1.0;
        self.z1_l = 0.0;
        self.z1_r = 0.0;
        self.head = 0;
        self.delay_l.fill(0.0);
        self.delay_r.fill(0.0);
    }

    pub fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        threshold_db: f32,
        ceiling_db: f32,
    ) {
        let frames = left.len().min(right.len());
        if frames == 0 || self.delay_l.is_empty() || self.delay_r.is_empty() {
            return;
        }
        let threshold_db = if threshold_db.is_finite() {
            threshold_db
        } else {
            -0.1
        };
        let ceiling_db = if ceiling_db.is_finite() {
            ceiling_db
        } else {
            -0.1
        };
        let threshold = (10.0_f32.powf(threshold_db / 20.0)).clamp(1.0e-6, 1.0);
        let ceiling = (10.0_f32.powf(ceiling_db / 20.0)).clamp(1.0e-6, 1.0);
        let capacity = self.delay_l.len();

        for frame in 0..frames {
            let input_l = if left[frame].is_finite() {
                left[frame]
            } else {
                0.0
            };
            let input_r = if right[frame].is_finite() {
                right[frame]
            } else {
                0.0
            };
            let q1_l = self.z1_l + (input_l - self.z1_l) * 0.25;
            let q2_l = self.z1_l + (input_l - self.z1_l) * 0.50;
            let q3_l = self.z1_l + (input_l - self.z1_l) * 0.75;
            let q1_r = self.z1_r + (input_r - self.z1_r) * 0.25;
            let q2_r = self.z1_r + (input_r - self.z1_r) * 0.50;
            let q3_r = self.z1_r + (input_r - self.z1_r) * 0.75;
            let peak_l = input_l
                .abs()
                .max(q1_l.abs())
                .max(q2_l.abs())
                .max(q3_l.abs());
            let peak_r = input_r
                .abs()
                .max(q1_r.abs())
                .max(q2_r.abs())
                .max(q3_r.abs());
            self.z1_l = input_l;
            self.z1_r = input_r;
            let peak = peak_l.max(peak_r);
            let target_gain = if peak > threshold {
                threshold / (peak + 1.0e-9)
            } else {
                1.0
            };
            if target_gain < self.current_gain {
                self.current_gain = target_gain;
            } else {
                self.current_gain += (target_gain - self.current_gain) * 0.001;
            }

            // Keep the native FastDelay ordering and one-sample ring latency:
            // read the previous delayed slot, then write the current input.
            let read_index = (self.head + capacity - 1 - self.delay_samples as usize) % capacity;
            let delayed_l = self.delay_l[read_index];
            let delayed_r = self.delay_r[read_index];
            self.delay_l[self.head] = input_l;
            self.delay_r[self.head] = input_r;
            self.head = (self.head + 1) % capacity;

            let output_l = delayed_l * self.current_gain * ceiling;
            let output_r = delayed_r * self.current_gain * ceiling;
            left[frame] = if output_l.is_finite() {
                output_l.clamp(-ceiling, ceiling)
            } else {
                0.0
            };
            right[frame] = if output_r.is_finite() {
                output_r.clamp(-ceiling, ceiling)
            } else {
                0.0
            };
        }
    }

    pub fn get_tail_samples(&self) -> u32 {
        let rate = if self.sample_rate.is_finite() && self.sample_rate > 0.0 {
            self.sample_rate
        } else {
            44_100.0
        };
        ((self.delay_samples as f64 + 0.35 * rate).min(30.0 * rate)) as u32
    }

    pub fn audit_true_peak_limiter(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.delay_samples > 0
            && self.delay_l.len() == self.delay_samples as usize + 1
            && self.delay_r.len() == self.delay_l.len()
            && self.head < self.delay_l.len()
            && self
                .delay_l
                .iter()
                .chain(&self.delay_r)
                .all(|sample| sample.is_finite())
            && self.current_gain.is_finite()
            && (0.0..=1.0).contains(&self.current_gain)
            && self.z1_l.is_finite()
            && self.z1_r.is_finite()
    }
}

#[no_mangle]
pub extern "C" fn hirari_true_peak_limiter_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(TruePeakLimiterState::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_true_peak_limiter_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<TruePeakLimiterState>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_true_peak_limiter_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = state.cast::<TruePeakLimiterState>().as_ref() {
        state.prepare(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_true_peak_limiter_reset(state: *mut c_void) {
    if let Some(state) = state.cast::<TruePeakLimiterState>().as_ref() {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_true_peak_limiter_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) {
    if left.is_null() || right.is_null() {
        return;
    }
    let Some(state) = state.cast::<TruePeakLimiterState>().as_ref() else {
        return;
    };
    state.process(
        std::slice::from_raw_parts_mut(left, frames),
        std::slice::from_raw_parts_mut(right, frames),
    );
}

#[no_mangle]
pub unsafe extern "C" fn hirari_true_peak_limiter_set_parameter(
    state: *const c_void,
    id: u32,
    value: f32,
) {
    if let Some(state) = state.cast::<TruePeakLimiterState>().as_ref() {
        state.set_parameter(id, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_true_peak_limiter_get_parameter(
    state: *const c_void,
    id: u32,
) -> f32 {
    state
        .cast::<TruePeakLimiterState>()
        .as_ref()
        .map_or(0.0, |state| state.parameter(id))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_true_peak_limiter_write_state(
    state: *const c_void,
    output: *mut u8,
    output_size: usize,
) -> bool {
    let Some(state) = state.cast::<TruePeakLimiterState>().as_ref() else {
        return false;
    };
    if output.is_null() || output_size != 8 {
        return false;
    }
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&state.parameter(0).to_ne_bytes());
    bytes[4..].copy_from_slice(&state.parameter(1).to_ne_bytes());
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len());
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_true_peak_limiter_restore_state(
    state: *const c_void,
    input: *const u8,
    input_size: usize,
) -> bool {
    let Some(state) = state.cast::<TruePeakLimiterState>().as_ref() else {
        return false;
    };
    if input.is_null() || input_size != 8 {
        return false;
    }
    let bytes = std::slice::from_raw_parts(input, input_size);
    let threshold = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
    let ceiling = f32::from_ne_bytes(bytes[4..8].try_into().unwrap());
    if !threshold.is_finite()
        || !ceiling.is_finite()
        || !(0.0..=1.0).contains(&threshold)
        || !(0.0..=1.0).contains(&ceiling)
    {
        return false;
    }
    state.set_parameter(0, threshold);
    state.set_parameter(1, ceiling);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_true_peak_limiter_latency(state: *const c_void) -> u32 {
    state
        .cast::<TruePeakLimiterState>()
        .as_ref()
        .map_or(0, |state| state.latency.load(Ordering::Relaxed))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_true_peak_limiter_tail(state: *const c_void) -> u32 {
    state
        .cast::<TruePeakLimiterState>()
        .as_ref()
        .map_or(0, |state| state.tail.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_audit_catches_corrupted_state_and_processes_finite_audio() {
        let mut limiter = TruePeakLimiterEngine::new(48_000.0);
        assert!(limiter.audit_true_peak_limiter());
        let mut left = vec![2.0; 64];
        let mut right = vec![2.0; 64];
        limiter.process(&mut left, &mut right, -1.0, -1.0);
        assert!(left.iter().all(|sample| sample.is_finite()));
        limiter.current_gain = f32::NAN;
        assert!(!limiter.audit_true_peak_limiter());
    }

    #[test]
    fn ffi_parameters_and_state_are_rust_owned_and_transactional() {
        let state = TruePeakLimiterState::new(48_000.0);
        let state_ptr = (&state as *const TruePeakLimiterState).cast();
        assert!((state.parameter(0) - ((-0.1f32 + 60.0) / 60.0)).abs() < f32::EPSILON);
        unsafe {
            hirari_true_peak_limiter_set_parameter(state_ptr, 0, 2.0);
            hirari_true_peak_limiter_set_parameter(state_ptr, 1, 0.25);
            hirari_true_peak_limiter_set_parameter(state_ptr, 0, f32::NAN);
        }
        assert_eq!(state.parameter(0), 1.0);
        assert_eq!(state.parameter(1), 0.25);

        let mut bytes = [0u8; 8];
        assert!(unsafe {
            hirari_true_peak_limiter_write_state(state_ptr, bytes.as_mut_ptr(), bytes.len())
        });
        let restored = TruePeakLimiterState::new(44_100.0);
        let restored_ptr = (&restored as *const TruePeakLimiterState).cast();
        assert!(unsafe {
            hirari_true_peak_limiter_restore_state(restored_ptr, bytes.as_ptr(), bytes.len())
        });
        assert_eq!(restored.parameter(0), 1.0);
        assert_eq!(restored.parameter(1), 0.25);

        bytes[0] ^= 0xff;
        let before = restored.parameter(0);
        assert!(!unsafe {
            hirari_true_peak_limiter_restore_state(restored_ptr, bytes.as_ptr(), bytes.len())
        });
        assert_eq!(restored.parameter(0), before);
    }

    #[test]
    fn ffi_processing_matches_the_engine_with_the_same_limiter_parameters() {
        let state = TruePeakLimiterState::new(48_000.0);
        state.set_parameter(0, 0.9);
        state.set_parameter(1, 59.0 / 60.0);
        let threshold_db = f32::from_bits(state.threshold_db.load(Ordering::Relaxed));
        let ceiling_db = f32::from_bits(state.ceiling_db.load(Ordering::Relaxed));
        let mut reference = TruePeakLimiterEngine::new(48_000.0);
        let mut ffi_left = (0..1024)
            .map(|frame| (frame as f32 * 0.037).sin() * 1.3)
            .collect::<Vec<_>>();
        let mut ffi_right = (0..1024)
            .map(|frame| (frame as f32 * 0.053).cos() * 1.1)
            .collect::<Vec<_>>();
        let mut reference_left = ffi_left.clone();
        let mut reference_right = ffi_right.clone();
        reference.process(
            &mut reference_left,
            &mut reference_right,
            threshold_db,
            ceiling_db,
        );
        unsafe {
            hirari_true_peak_limiter_process(
                (&state as *const TruePeakLimiterState).cast_mut().cast(),
                ffi_left.as_mut_ptr(),
                ffi_right.as_mut_ptr(),
                ffi_left.len(),
            );
        }
        assert_eq!(ffi_left, reference_left);
        assert_eq!(ffi_right, reference_right);

        state.prepare(96_000.0);
        assert_eq!(state.latency.load(Ordering::Relaxed), 144);
        assert_eq!(
            state.tail.load(Ordering::Relaxed),
            (144.0 + 0.35 * 96_000.0) as u32
        );
    }
}
