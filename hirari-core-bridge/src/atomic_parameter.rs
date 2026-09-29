use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

const LINEAR: u8 = 0;
const EXPONENTIAL: u8 = 1;
const NONE: u8 = 2;
const UNIPOLAR: u8 = 0;
const BIPOLAR: u8 = 1;

pub(crate) struct AtomicParameterState {
    target: AtomicU32,
    ai_offset: AtomicU32,
    reset_value: AtomicU32,
    should_reset: AtomicBool,
    dirty: AtomicBool,
    current: AtomicU32,
    coefficient: AtomicU32,
    step: AtomicU32,
    display_mode: AtomicU8,
    unit: AtomicU8,
    smoothing_type: AtomicU8,
    sample_rate: AtomicU64,
    smoothing_time_ms: AtomicU64,
}

impl AtomicParameterState {
    pub(crate) fn new(initial: f32, display_mode: u8) -> Self {
        Self {
            target: AtomicU32::new(initial.to_bits()),
            ai_offset: AtomicU32::new(0.0_f32.to_bits()),
            reset_value: AtomicU32::new(0.0_f32.to_bits()),
            should_reset: AtomicBool::new(false),
            dirty: AtomicBool::new(true),
            current: AtomicU32::new(initial.to_bits()),
            coefficient: AtomicU32::new(0.01_f32.to_bits()),
            step: AtomicU32::new(0.0_f32.to_bits()),
            display_mode: AtomicU8::new(display_mode.min(BIPOLAR)),
            unit: AtomicU8::new(0),
            smoothing_type: AtomicU8::new(EXPONENTIAL),
            sample_rate: AtomicU64::new(44100.0_f64.to_bits()),
            smoothing_time_ms: AtomicU64::new(10.0_f64.to_bits()),
        }
    }

    pub(crate) fn get_next_value(&self) -> f32 {
        if self.dirty.load(Ordering::Acquire) {
            self.update_internal_state();
        }
        let target = self.load_f32(&self.target, Ordering::Relaxed);
        let mut current = self.load_f32(&self.current, Ordering::Relaxed);
        if (current - target).abs() < 1.0e-7 {
            current = target;
            self.store_f32(&self.current, current, Ordering::Relaxed);
            return self.clamp_normalized(
                self.normalized_for(current) + self.load_f32(&self.ai_offset, Ordering::Relaxed),
            );
        }
        match self.smoothing_type.load(Ordering::Relaxed) {
            EXPONENTIAL => {
                current += (target - current) * self.load_f32(&self.coefficient, Ordering::Relaxed);
                if (current - target).abs() < 1.0e-24 {
                    current = target;
                }
            }
            LINEAR => {
                let step = self.load_f32(&self.step, Ordering::Relaxed);
                current += step;
                if (step > 0.0 && current > target) || (step < 0.0 && current < target) {
                    current = target;
                }
            }
            _ => current = target,
        }
        self.store_f32(&self.current, current, Ordering::Relaxed);
        self.clamp_normalized(
            self.normalized_for(current) + self.load_f32(&self.ai_offset, Ordering::Relaxed),
        )
    }

    unsafe fn get_next_block(&self, buffer: *mut f32, length: usize) {
        if self.dirty.load(Ordering::Acquire) {
            self.update_internal_state();
        }
        if buffer.is_null() || length == 0 {
            return;
        }
        let target = self.load_f32(&self.target, Ordering::Relaxed);
        let smoothing_type = self.smoothing_type.load(Ordering::Relaxed);
        let ai_mod = self.load_f32(&self.ai_offset, Ordering::Relaxed);
        let display_mode = self.display_mode.load(Ordering::Relaxed);
        let current_scale = if display_mode == UNIPOLAR { 0.5 } else { 1.0 };
        let offset = if display_mode == UNIPOLAR { 1.0 } else { 0.0 };
        let mut current = self.load_f32(&self.current, Ordering::Relaxed);

        if (current - target).abs() < 1.0e-7 {
            current = target;
            self.store_f32(&self.current, current, Ordering::Relaxed);
            let value = self.clamp_normalized((current + offset) * current_scale + ai_mod);
            for index in 0..length {
                buffer.add(index).write(value);
            }
        } else if smoothing_type == LINEAR {
            for index in 0..length {
                let step = self.load_f32(&self.step, Ordering::Relaxed);
                current += step;
                if (step > 0.0 && current > target) || (step < 0.0 && current < target) {
                    current = target;
                }
                buffer
                    .add(index)
                    .write(self.clamp_normalized((current + offset) * current_scale + ai_mod));
            }
            self.store_f32(&self.current, current, Ordering::Relaxed);
        } else {
            for index in 0..length {
                current += (target - current) * self.load_f32(&self.coefficient, Ordering::Relaxed);
                if (current - target).abs() < 1.0e-24 {
                    current = target;
                }
                buffer
                    .add(index)
                    .write(self.clamp_normalized((current + offset) * current_scale + ai_mod));
            }
            if (current - target).abs() < 1.0e-7 {
                current = target;
            }
            self.store_f32(&self.current, current, Ordering::Relaxed);
        }
    }

    fn update_internal_state(&self) {
        if self.should_reset.swap(false, Ordering::AcqRel) {
            let reset_value = self.load_f32(&self.reset_value, Ordering::Acquire);
            self.store_f32(&self.current, reset_value, Ordering::Relaxed);
        }
        self.recalculate_coefficients();
        self.dirty.store(false, Ordering::Release);
    }

    fn recalculate_coefficients(&self) {
        let sample_rate = f64::from_bits(self.sample_rate.load(Ordering::Relaxed));
        let smoothing_time_ms = f64::from_bits(self.smoothing_time_ms.load(Ordering::Relaxed));
        if sample_rate > 0.0 {
            let samples = smoothing_time_ms * 0.001 * sample_rate;
            self.store_f32(
                &self.coefficient,
                (1.0 - (-1.0 / samples.max(1.0)).exp()) as f32,
                Ordering::Release,
            );
            self.update_step(sample_rate, smoothing_time_ms);
        }
    }

    fn update_step(&self, sample_rate: f64, smoothing_time_ms: f64) {
        let target = self.load_f32(&self.target, Ordering::Relaxed);
        let current = self.load_f32(&self.current, Ordering::Relaxed);
        let samples = smoothing_time_ms * 0.001 * sample_rate;
        let step = if samples.is_finite() && samples > 0.0 {
            (target - current) / samples.max(1.0) as f32
        } else {
            0.0
        };
        self.store_f32(
            &self.step,
            if step.is_finite() { step } else { 0.0 },
            Ordering::Release,
        );
    }

    fn normalized_value(&self) -> f32 {
        let current = self.load_f32(&self.current, Ordering::Relaxed);
        self.normalized_for(current)
    }

    fn normalized_for(&self, current: f32) -> f32 {
        if self.display_mode.load(Ordering::Relaxed) == UNIPOLAR {
            ((current + 1.0) * 0.5).clamp(0.0, 1.0)
        } else {
            current
        }
    }

    fn clamp_normalized(&self, value: f32) -> f32 {
        value.clamp(0.0, 1.0)
    }

    fn load_f32(&self, value: &AtomicU32, ordering: Ordering) -> f32 {
        f32::from_bits(value.load(ordering))
    }

    fn store_f32(&self, target: &AtomicU32, value: f32, ordering: Ordering) {
        target.store(value.to_bits(), ordering);
    }

    pub(crate) fn set_target(&self, value: f32) {
        if value.is_finite() {
            self.store_f32(&self.target, value, Ordering::Release);
            self.dirty.store(true, Ordering::Release);
        }
    }

    pub(crate) fn set_sample_rate(&self, sample_rate: f64) {
        if sample_rate.is_finite() && sample_rate > 0.0 {
            self.sample_rate
                .store(sample_rate.to_bits(), Ordering::Relaxed);
            self.recalculate_coefficients();
        }
    }

    pub(crate) fn set_smoothing_time(&self, milliseconds: f64) {
        if milliseconds.is_finite() && milliseconds >= 0.0 {
            self.smoothing_time_ms
                .store(milliseconds.to_bits(), Ordering::Relaxed);
            self.recalculate_coefficients();
        }
    }

    pub(crate) fn reset_to_target(&self) {
        let target = self.load_f32(&self.target, Ordering::Acquire);
        self.store_f32(&self.current, target, Ordering::Release);
        self.dirty.store(false, Ordering::Release);
    }
}

#[no_mangle]
pub extern "C" fn hirari_atomic_parameter_create(initial: f32, display_mode: u8) -> *mut c_void {
    Box::into_raw(Box::new(AtomicParameterState::new(initial, display_mode))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atomic_parameter_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<AtomicParameterState>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atomic_parameter_get_target(state: *const c_void) -> f32 {
    state
        .cast::<AtomicParameterState>()
        .as_ref()
        .map_or(0.0, |state| {
            state.load_f32(&state.target, Ordering::Acquire)
        })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atomic_parameter_set_target(state: *const c_void, value: f32) {
    if !value.is_finite() {
        return;
    }
    if let Some(state) = state.cast::<AtomicParameterState>().as_ref() {
        state.set_target(value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atomic_parameter_get_next(state: *const c_void) -> f32 {
    state
        .cast::<AtomicParameterState>()
        .as_ref()
        .map_or(0.0, AtomicParameterState::get_next_value)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atomic_parameter_get_block(
    state: *const c_void,
    buffer: *mut f32,
    length: usize,
) {
    if let Some(state) = state.cast::<AtomicParameterState>().as_ref() {
        state.get_next_block(buffer, length);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atomic_parameter_get_normalized(state: *const c_void) -> f32 {
    state
        .cast::<AtomicParameterState>()
        .as_ref()
        .map_or(0.0, AtomicParameterState::normalized_value)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atomic_parameter_set_unit(state: *const c_void, unit: u8) {
    if let Some(state) = state.cast::<AtomicParameterState>().as_ref() {
        state.unit.store(unit.min(4), Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atomic_parameter_set_display_mode(state: *const c_void, mode: u8) {
    if let Some(state) = state.cast::<AtomicParameterState>().as_ref() {
        state
            .display_mode
            .store(mode.min(BIPOLAR), Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atomic_parameter_set_sample_rate(
    state: *const c_void,
    sample_rate: f64,
) {
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return;
    }
    if let Some(state) = state.cast::<AtomicParameterState>().as_ref() {
        state.set_sample_rate(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atomic_parameter_set_smoothing_time(
    state: *const c_void,
    milliseconds: f64,
) {
    if !milliseconds.is_finite() || milliseconds < 0.0 {
        return;
    }
    if let Some(state) = state.cast::<AtomicParameterState>().as_ref() {
        state.set_smoothing_time(milliseconds);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atomic_parameter_set_ai_modulation(
    state: *const c_void,
    offset: f32,
) {
    if let Some(state) = state.cast::<AtomicParameterState>().as_ref() {
        state.store_f32(&state.ai_offset, offset, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atomic_parameter_reset_to_target(state: *const c_void) {
    if let Some(state) = state.cast::<AtomicParameterState>().as_ref() {
        state.reset_to_target();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atomic_parameter_set_reset_value(state: *const c_void, value: f32) {
    if let Some(state) = state.cast::<AtomicParameterState>().as_ref() {
        state.store_f32(&state.reset_value, value, Ordering::Release);
        state.should_reset.store(true, Ordering::Release);
        state.dirty.store(true, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atomic_parameter_set_smoothing_type(
    state: *const c_void,
    smoothing_type: u8,
) {
    if let Some(state) = state.cast::<AtomicParameterState>().as_ref() {
        state
            .smoothing_type
            .store(smoothing_type.min(NONE), Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atomic_parameter_get_value_string(
    state: *const c_void,
    buffer: *mut c_char,
    size: usize,
) {
    if buffer.is_null() || size == 0 {
        return;
    }
    let Some(state) = state.cast::<AtomicParameterState>().as_ref() else {
        buffer.write(0);
        return;
    };
    let value = state.load_f32(&state.target, Ordering::Acquire);
    let text = match state.unit.load(Ordering::Acquire) {
        0 => format!("{:.1}%", (value + 1.0) * 50.0),
        1 => {
            let db = 20.0 * ((value + 1.0) * 0.5).max(1.0e-5).log10();
            if db < -90.0 {
                "-inf dB".to_owned()
            } else {
                format!("{db:.1} dB")
            }
        }
        2 => {
            let frequency = 20.0 * 1000.0_f32.powf((value + 1.0) * 0.5);
            if frequency >= 1000.0 {
                format!("{:.2} kHz", frequency * 0.001)
            } else {
                format!("{frequency:.0} Hz")
            }
        }
        3 => format!("{:.1} ms", ((value + 1.0) * 500.0).max(0.0)),
        _ => format!("{value:.2}"),
    };
    let output = std::slice::from_raw_parts_mut(buffer.cast::<u8>(), size);
    let count = text.len().min(size - 1);
    output[..count].copy_from_slice(&text.as_bytes()[..count]);
    output[count] = 0;
}
