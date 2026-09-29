use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

struct FilterRuntime {
    sample_rate: f64,
    env: f32,
    attack: f32,
    release: f32,
    s1: [f32; 2],
    s2: [f32; 2],
}

/// Rust-owned sample-accurate state for the native AutoFilter processor.
/// Control values are atomic; runtime state is accessed only by the audio
/// thread, with prepare/reset serialized against process just as in the host.
pub struct AutoFilterEngine {
    cutoff_base: AtomicU32,
    resonance: AtomicU32,
    sensitivity: AtomicU32,
    runtime: UnsafeCell<FilterRuntime>,
}

impl AutoFilterEngine {
    fn new() -> Self {
        Self {
            cutoff_base: AtomicU32::new(0.2f32.to_bits()),
            resonance: AtomicU32::new(0.3f32.to_bits()),
            sensitivity: AtomicU32::new(0.8f32.to_bits()),
            runtime: UnsafeCell::new(FilterRuntime {
                sample_rate: 44_100.0,
                env: 0.0,
                attack: 0.0,
                release: 0.0,
                s1: [0.0; 2],
                s2: [0.0; 2],
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
        runtime.attack = (1.0 - (-1.0 / (0.005 * runtime.sample_rate)).exp()) as f32;
        runtime.release = (1.0 - (-1.0 / (0.100 * runtime.sample_rate)).exp()) as f32;
    }

    fn reset(&self) {
        let runtime = unsafe { &mut *self.runtime.get() };
        runtime.env = 0.0;
        runtime.s1 = [0.0; 2];
        runtime.s2 = [0.0; 2];
    }

    fn set_parameter(&self, id: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        let target = match id {
            0 => &self.cutoff_base,
            1 => &self.resonance,
            2 => &self.sensitivity,
            _ => return,
        };
        target.store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    fn get_parameter(&self, id: u32) -> f32 {
        let source = match id {
            0 => &self.cutoff_base,
            1 => &self.resonance,
            2 => &self.sensitivity,
            _ => return 0.0,
        };
        f32::from_bits(source.load(Ordering::Relaxed))
    }

    unsafe fn process(&self, channels: *mut *mut f32, channel_count: u32, frames: usize, mix: f32) {
        if channels.is_null() || channel_count == 0 || frames == 0 {
            return;
        }
        let channel_count = channel_count.min(2) as usize;
        let pointers = std::slice::from_raw_parts(channels, channel_count);
        if pointers.iter().any(|pointer| pointer.is_null()) {
            return;
        }

        let runtime = &mut *self.runtime.get();
        let cutoff_base = f32::from_bits(self.cutoff_base.load(Ordering::Relaxed));
        let resonance = f32::from_bits(self.resonance.load(Ordering::Relaxed));
        let sensitivity = f32::from_bits(self.sensitivity.load(Ordering::Relaxed));
        let mix = if mix.is_finite() {
            mix.clamp(0.0, 1.0)
        } else {
            1.0
        };

        for frame in 0..frames {
            let input_left = *pointers[0].add(frame);
            let input_right = if channel_count > 1 {
                *pointers[1].add(frame)
            } else {
                input_left
            };
            let detector = 0.5 * (input_left.abs() + input_right.abs());
            let coeff = if detector > runtime.env {
                runtime.attack
            } else {
                runtime.release
            };
            runtime.env += (detector - runtime.env) * coeff;

            let cutoff_norm = (cutoff_base + runtime.env * sensitivity * 0.7).clamp(0.01, 0.49);
            let f = 2.0 * (std::f32::consts::PI * cutoff_norm).sin();
            let damp = (2.0 * (1.0 - resonance.powf(0.25))).clamp(0.05, 2.0);

            for channel in 0..channel_count {
                let input = if channel == 0 {
                    input_left
                } else {
                    input_right
                };
                let low = runtime.s1[channel] + f * runtime.s2[channel];
                let high = input - low - damp * runtime.s2[channel];
                let band = f * high + runtime.s2[channel];
                runtime.s1[channel] = low;
                runtime.s2[channel] = band;
                let wet = low + band * 0.35;
                *pointers[channel].add(frame) = input * (1.0 - mix) + wet * mix;
            }
        }
    }
}

// The host serializes lifecycle operations; only the three parameter atomics
// may be changed concurrently with audio processing.
unsafe impl Sync for AutoFilterEngine {}

#[no_mangle]
pub extern "C" fn hirari_auto_filter_create() -> *mut c_void {
    Box::into_raw(Box::new(AutoFilterEngine::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_auto_filter_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<AutoFilterEngine>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_auto_filter_set_sample_rate(
    state: *const c_void,
    sample_rate: f64,
) {
    if let Some(state) = state.cast::<AutoFilterEngine>().as_ref() {
        state.set_sample_rate(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_auto_filter_reset(state: *const c_void) {
    if let Some(state) = state.cast::<AutoFilterEngine>().as_ref() {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_auto_filter_set_parameter(
    state: *const c_void,
    id: u32,
    value: f32,
) {
    if let Some(state) = state.cast::<AutoFilterEngine>().as_ref() {
        state.set_parameter(id, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_auto_filter_get_parameter(state: *const c_void, id: u32) -> f32 {
    state
        .cast::<AutoFilterEngine>()
        .as_ref()
        .map_or(0.0, |state| state.get_parameter(id))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_auto_filter_process(
    state: *const c_void,
    channels: *mut *mut f32,
    channel_count: u32,
    frames: usize,
    mix: f32,
) {
    if let Some(state) = state.cast::<AutoFilterEngine>().as_ref() {
        state.process(channels, channel_count, frames, mix);
    }
}
