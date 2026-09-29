use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy)]
struct SmoothedSvf {
    sample_rate: f64,
    frequency: f32,
    resonance: f32,
    last_frequency: f32,
    last_resonance: f32,
    g: f32,
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    ic1: [f32; 2],
    ic2: [f32; 2],
}

impl SmoothedSvf {
    fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            frequency: 1000.0,
            resonance: 0.707,
            last_frequency: -1.0,
            last_resonance: -1.0,
            g: 0.0,
            k: 0.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            ic1: [0.0; 2],
            ic2: [0.0; 2],
        }
    }

    fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
    }

    fn reset(&mut self) {
        self.ic1 = [0.0; 2];
        self.ic2 = [0.0; 2];
    }

    fn process_lp(&mut self, input: f32, channel: usize, target_frequency: f32) -> f32 {
        // The C++ implementation uses the Rust-backed ParameterSmoother with
        // its default 0.01 coefficient and advances both controls per sample.
        self.frequency += (target_frequency - self.frequency) * 0.01;
        self.resonance += (0.707 - self.resonance) * 0.01;
        if (self.frequency - self.last_frequency).abs() > 0.001
            || (self.resonance - self.last_resonance).abs() > 0.001
        {
            let safe_sample_rate = self.sample_rate.max(1000.0) as f32;
            let safe_frequency = if self.frequency.is_finite() {
                self.frequency.clamp(5.0, safe_sample_rate * 0.49)
            } else {
                1000.0f32.clamp(5.0, safe_sample_rate * 0.49)
            };
            let safe_resonance = if self.resonance.is_finite() {
                self.resonance.clamp(0.05, 4.0)
            } else {
                0.707
            };
            let g = (std::f32::consts::PI * safe_frequency / safe_sample_rate).tan();
            let k = 1.0 / safe_resonance;
            self.g = g;
            self.k = k;
            self.a1 = 1.0 / (1.0 + g * (g + k));
            self.a2 = g * self.a1;
            self.a3 = g * self.a2;
            if !self.a1.is_finite() || !self.a2.is_finite() || !self.a3.is_finite() {
                self.a1 = 1.0;
                self.a2 = 0.0;
                self.a3 = 0.0;
                self.g = 0.0;
                self.k = 1.0;
            }
            self.last_frequency = safe_frequency;
            self.last_resonance = safe_resonance;
        }

        let v3 = input - self.ic2[channel];
        let v1 = self.a1 * self.ic1[channel] + self.a2 * v3;
        let v2 = self.ic2[channel] + self.a2 * self.ic1[channel] + self.a3 * v3;
        self.ic1[channel] = 2.0 * v1 - self.ic1[channel];
        self.ic2[channel] = 2.0 * v2 - self.ic2[channel];
        v2
    }
}

struct ExciterRuntime {
    sample_rate: f64,
    low_pass: [SmoothedSvf; 2],
    mid_low_pass: [SmoothedSvf; 2],
}

/// Rust-owned multiband exciter DSP. Crossover targets may be changed from
/// the control thread while audio processing advances each filter's smoother.
pub struct MultibandExciterEngine {
    low_cut_bits: AtomicU32,
    high_cut_bits: AtomicU32,
    runtime: UnsafeCell<ExciterRuntime>,
}

impl MultibandExciterEngine {
    fn new(sample_rate: f64) -> Self {
        let sample_rate = if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate)
        {
            sample_rate
        } else {
            44_100.0
        };
        Self {
            low_cut_bits: AtomicU32::new(200.0f32.to_bits()),
            high_cut_bits: AtomicU32::new(3000.0f32.to_bits()),
            runtime: UnsafeCell::new(ExciterRuntime {
                sample_rate,
                low_pass: [SmoothedSvf::new(sample_rate), SmoothedSvf::new(sample_rate)],
                mid_low_pass: [SmoothedSvf::new(sample_rate), SmoothedSvf::new(sample_rate)],
            }),
        }
    }

    fn set_sample_rate(&self, sample_rate: f64) {
        let runtime = unsafe { &mut *self.runtime.get() };
        runtime.sample_rate =
            if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
                sample_rate
            } else {
                44_100.0
            };
        let sample_rate = runtime.sample_rate;
        for filter in runtime
            .low_pass
            .iter_mut()
            .chain(runtime.mid_low_pass.iter_mut())
        {
            filter.set_sample_rate(sample_rate);
        }
    }

    fn setup_crossover(&self, low_cut: f32, high_cut: f32) {
        if low_cut.is_finite() {
            self.low_cut_bits
                .store(low_cut.to_bits(), Ordering::Relaxed);
        }
        if high_cut.is_finite() {
            self.high_cut_bits
                .store(high_cut.to_bits(), Ordering::Relaxed);
        }
    }

    fn reset(&self) {
        let runtime = unsafe { &mut *self.runtime.get() };
        for filter in runtime
            .low_pass
            .iter_mut()
            .chain(runtime.mid_low_pass.iter_mut())
        {
            filter.reset();
        }
    }

    unsafe fn process(&self, left: *mut f32, right: *mut f32, frames: usize) {
        if left.is_null() || right.is_null() {
            return;
        }
        let runtime = &mut *self.runtime.get();

        for frame in 0..frames {
            let input_left_raw = *left.add(frame);
            let input_right_raw = *right.add(frame);
            let input_left = if input_left_raw.is_finite() {
                input_left_raw
            } else {
                0.0
            };
            let input_right = if input_right_raw.is_finite() {
                input_right_raw
            } else {
                0.0
            };

            // Each C++ LinearSmoother loads its target when that filter
            // advances, preserving control changes that arrive mid-block.
            let low_target_left = f32::from_bits(self.low_cut_bits.load(Ordering::Relaxed));
            let low_left = runtime.low_pass[0].process_lp(input_left, 0, low_target_left);
            let low_target_right = f32::from_bits(self.low_cut_bits.load(Ordering::Relaxed));
            let low_right = runtime.low_pass[1].process_lp(input_right, 1, low_target_right);
            let high_target_left = f32::from_bits(self.high_cut_bits.load(Ordering::Relaxed));
            let mid_left =
                runtime.mid_low_pass[0].process_lp(input_left, 0, high_target_left) - low_left;
            let high_target_right = f32::from_bits(self.high_cut_bits.load(Ordering::Relaxed));
            let mid_right =
                runtime.mid_low_pass[1].process_lp(input_right, 1, high_target_right) - low_right;
            let high_left = input_left - low_left - mid_left;
            let high_right = input_right - low_right - mid_right;
            let excite_left =
                low_left + 0.35 * (mid_left * 1.4).tanh() + 0.5 * (high_left * 2.0).tanh();
            let excite_right =
                low_right + 0.35 * (mid_right * 1.4).tanh() + 0.5 * (high_right * 2.0).tanh();
            let output_left = 0.75 * input_left + 0.25 * excite_left;
            let output_right = 0.75 * input_right + 0.25 * excite_right;

            if left == right {
                let mono = 0.5 * (output_left + output_right);
                *left.add(frame) = if mono.is_finite() { mono } else { 0.0 };
            } else {
                *left.add(frame) = if output_left.is_finite() {
                    output_left
                } else {
                    0.0
                };
                *right.add(frame) = if output_right.is_finite() {
                    output_right
                } else {
                    0.0
                };
            }
        }
    }
}

unsafe impl Sync for MultibandExciterEngine {}

#[no_mangle]
pub extern "C" fn hirari_multiband_exciter_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(MultibandExciterEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_multiband_exciter_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<MultibandExciterEngine>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_multiband_exciter_set_sample_rate(
    state: *const c_void,
    sample_rate: f64,
) {
    if let Some(state) = state.cast::<MultibandExciterEngine>().as_ref() {
        state.set_sample_rate(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_multiband_exciter_setup_crossover(
    state: *const c_void,
    low_cut: f32,
    high_cut: f32,
) {
    if let Some(state) = state.cast::<MultibandExciterEngine>().as_ref() {
        state.setup_crossover(low_cut, high_cut);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_multiband_exciter_reset(state: *const c_void) {
    if let Some(state) = state.cast::<MultibandExciterEngine>().as_ref() {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_multiband_exciter_process(
    state: *const c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
    if let Some(state) = state.cast::<MultibandExciterEngine>().as_ref() {
        state.process(left, right, frames as usize);
    }
}
