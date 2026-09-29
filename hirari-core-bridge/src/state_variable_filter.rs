use crate::parameter_smoother::SmootherOrchestrator;
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};

struct FilterRuntime {
    ic1: f32,
    ic2: f32,
    last_frequency: f32,
    last_resonance: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    g: f32,
    k: f32,
}

/// Rust-owned zero-delay-feedback SVF. Parameter targets are lock-free and
/// audio state is owned by the serialized realtime/lifecycle path.
pub struct StateVariableFilterEngine {
    sample_rate_bits: AtomicU64,
    frequency: SmootherOrchestrator,
    resonance: SmootherOrchestrator,
    runtime: UnsafeCell<FilterRuntime>,
}

impl StateVariableFilterEngine {
    pub fn new(sample_rate: f64) -> Self {
        let frequency = SmootherOrchestrator::new(1000.0);
        frequency.reset(1000.0);
        let resonance = SmootherOrchestrator::new(0.707);
        resonance.reset(0.707);
        Self {
            sample_rate_bits: AtomicU64::new(
                if sample_rate.is_finite() && sample_rate > 1000.0 {
                    sample_rate
                } else {
                    44100.0
                }
                .to_bits(),
            ),
            frequency,
            resonance,
            runtime: UnsafeCell::new(FilterRuntime {
                ic1: 0.0,
                ic2: 0.0,
                last_frequency: -1.0,
                last_resonance: -1.0,
                a1: 0.0,
                a2: 0.0,
                a3: 0.0,
                g: 0.0,
                k: 0.0,
            }),
        }
    }

    pub fn reset(&self) {
        let runtime = unsafe { &mut *self.runtime.get() };
        runtime.ic1 = 0.0;
        runtime.ic2 = 0.0;
    }

    pub fn set_sample_rate(&self, sample_rate: f64) {
        if sample_rate.is_finite() && sample_rate > 1000.0 {
            self.sample_rate_bits
                .store(sample_rate.to_bits(), Ordering::Release);
        }
    }

    pub fn set_parameters(&self, frequency: f32, resonance: f32, _mode: i32) {
        self.frequency.set_target(frequency);
        self.resonance.set_target(resonance);
    }

    fn update_coefficients(&self) {
        let frequency = self.frequency.next_value();
        let resonance = self.resonance.next_value();
        let runtime = unsafe { &mut *self.runtime.get() };
        if (frequency - runtime.last_frequency).abs() > 0.001
            || (resonance - runtime.last_resonance).abs() > 0.001
        {
            let sample_rate = f64::from_bits(self.sample_rate_bits.load(Ordering::Acquire));
            let safe_sr = (if sample_rate.is_finite() {
                sample_rate
            } else {
                1000.0
            })
            .max(1000.0) as f32;
            let safe_frequency = if frequency.is_finite() {
                frequency
            } else {
                1000.0
            }
            .clamp(5.0, safe_sr * 0.49);
            let safe_resonance = if resonance.is_finite() {
                resonance
            } else {
                0.707
            }
            .clamp(0.05, 4.0);
            let mut g = (std::f32::consts::PI * safe_frequency / safe_sr).tan();
            let k = 1.0 / safe_resonance;
            runtime.a1 = 1.0 / (1.0 + g * (g + k));
            runtime.a2 = g * runtime.a1;
            runtime.a3 = g * runtime.a2;
            if !runtime.a1.is_finite() || !runtime.a2.is_finite() || !runtime.a3.is_finite() {
                runtime.a1 = 1.0;
                runtime.a2 = 0.0;
                runtime.a3 = 0.0;
                g = 0.0;
                runtime.k = 1.0;
            } else {
                runtime.k = k;
            }
            runtime.g = g;
            runtime.last_frequency = safe_frequency;
            runtime.last_resonance = safe_resonance;
        }
    }

    fn process_sample(&self, input: f32, mode: u32) -> f32 {
        self.update_coefficients();
        let runtime = unsafe { &mut *self.runtime.get() };
        if mode == 0 {
            let v3 = input - runtime.ic2;
            let v1 = runtime.a1 * runtime.ic1 + runtime.a2 * v3;
            let v2 = runtime.ic2 + runtime.a2 * runtime.ic1 + runtime.a3 * v3;
            runtime.ic1 = 2.0 * v1 - runtime.ic1;
            runtime.ic2 = 2.0 * v2 - runtime.ic2;
            return v2;
        }
        let g = runtime.g;
        let k = runtime.k;
        let h = 1.0 / (1.0 + g * (g + k));
        let bp = h * (runtime.ic1 + g * (input - runtime.ic2));
        let lp = runtime.ic2 + g * bp;
        runtime.ic1 = 2.0 * bp - runtime.ic1;
        runtime.ic2 = 2.0 * lp - runtime.ic2;
        if mode == 1 {
            bp
        } else {
            input - k * bp - lp
        }
    }

    pub unsafe fn process_block(&self, data: *mut f32, frames: u32, mode: u32) {
        if data.is_null() {
            return;
        }
        for index in 0..frames as usize {
            let input = unsafe { *data.add(index) };
            unsafe {
                *data.add(index) = self.process_sample(input, mode);
            }
        }
    }

    pub fn process_one(&self, input: f32, mode: u32) -> f32 {
        self.process_sample(input, mode)
    }
}

// The host serializes lifecycle operations; parameter targets are atomic.
unsafe impl Sync for StateVariableFilterEngine {}

#[no_mangle]
pub extern "C" fn hirari_svf_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(StateVariableFilterEngine::new(sample_rate))).cast()
}
#[no_mangle]
pub unsafe extern "C" fn hirari_svf_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<StateVariableFilterEngine>()) });
    }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_svf_reset(state: *const c_void) {
    if let Some(engine) = unsafe { state.cast::<StateVariableFilterEngine>().as_ref() } {
        engine.reset();
    }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_svf_set_sample_rate(state: *const c_void, sample_rate: f64) {
    if let Some(engine) = unsafe { state.cast::<StateVariableFilterEngine>().as_ref() } {
        engine.set_sample_rate(sample_rate);
    }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_svf_set_parameters(
    state: *const c_void,
    frequency: f32,
    resonance: f32,
    mode: i32,
) {
    if let Some(engine) = unsafe { state.cast::<StateVariableFilterEngine>().as_ref() } {
        engine.set_parameters(frequency, resonance, mode);
    }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_svf_process_block(
    state: *const c_void,
    data: *mut f32,
    frames: u32,
    mode: u32,
) {
    if let Some(engine) = unsafe { state.cast::<StateVariableFilterEngine>().as_ref() } {
        unsafe {
            engine.process_block(data, frames, mode);
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_svf_process_sample(
    state: *const c_void,
    input: f32,
    mode: u32,
) -> f32 {
    unsafe { state.cast::<StateVariableFilterEngine>().as_ref() }
        .map_or(0.0, |engine| engine.process_one(input, mode))
}
