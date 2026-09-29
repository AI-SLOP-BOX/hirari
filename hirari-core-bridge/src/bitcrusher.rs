use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

struct BitcrusherRuntime {
    hold_sample_l: f32,
    hold_sample_r: f32,
    sample_counter: f32,
}

pub struct BitcrusherEngine {
    bits: AtomicU32,
    downsample: AtomicU32,
    runtime: UnsafeCell<BitcrusherRuntime>,
}

impl Default for BitcrusherEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl BitcrusherEngine {
    pub fn new() -> Self {
        Self {
            bits: AtomicU32::new(16.0f32.to_bits()),
            downsample: AtomicU32::new(1.0f32.to_bits()),
            runtime: UnsafeCell::new(BitcrusherRuntime {
                hold_sample_l: 0.0,
                hold_sample_r: 0.0,
                sample_counter: 0.0,
            }),
        }
    }

    fn load(parameter: &AtomicU32) -> f32 {
        f32::from_bits(parameter.load(Ordering::Relaxed))
    }

    pub fn reset(&self) {
        // SAFETY: Reset must be serialized with audio processing by the host.
        let runtime = unsafe { &mut *self.runtime.get() };
        runtime.hold_sample_l = 0.0;
        runtime.hold_sample_r = 0.0;
        runtime.sample_counter = 0.0;
    }

    pub fn set_bits(&self, b: f32) {
        self.set_bits_native(b);
    }

    pub fn set_downsample(&self, d: f32) {
        self.set_downsample_native(d);
    }

    /// Rust API using the production C++ processor's full wet signal.
    pub fn process(&self, l: &mut [f32], r: &mut [f32]) {
        self.process_native(l, Some(r), 1.0);
    }

    pub fn set_bits_native(&self, bits: f32) {
        let bits = if bits.is_finite() {
            bits.clamp(1.0, 24.0)
        } else {
            16.0
        };
        self.bits.store(bits.to_bits(), Ordering::Relaxed);
    }

    pub fn set_downsample_native(&self, downsample: f32) {
        let downsample = if downsample.is_finite() {
            downsample.max(1.0)
        } else {
            1.0
        };
        self.downsample
            .store(downsample.to_bits(), Ordering::Relaxed);
    }

    pub fn process_native(&self, left: &mut [f32], right: Option<&mut [f32]>, mix: f32) {
        // SAFETY: One audio thread owns the runtime; UI parameter writes use atomics.
        let runtime = unsafe { &mut *self.runtime.get() };
        let bits = Self::load(&self.bits).clamp(1.0, 24.0) as i32;
        let levels = 2.0_f32.powi(bits - 1);
        let hold = Self::load(&self.downsample).ceil().max(1.0);
        let mix = if mix.is_finite() {
            mix.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let Some(right) = right else {
            for sample in left {
                if runtime.sample_counter <= 0.0 {
                    runtime.hold_sample_l = (*sample * levels).round() / levels;
                    runtime.hold_sample_r = runtime.hold_sample_l;
                    runtime.sample_counter = hold;
                }
                runtime.sample_counter -= 1.0;
                *sample = *sample * (1.0 - mix) + runtime.hold_sample_l * mix;
            }
            return;
        };
        let frames = left.len().min(right.len());
        for index in 0..frames {
            if runtime.sample_counter <= 0.0 {
                runtime.hold_sample_l = (left[index] * levels).round() / levels;
                runtime.hold_sample_r = (right[index] * levels).round() / levels;
                runtime.sample_counter = hold;
            }
            runtime.sample_counter -= 1.0;
            let dry_left = left[index];
            let dry_right = right[index];
            left[index] = dry_left * (1.0 - mix) + runtime.hold_sample_l * mix;
            right[index] = dry_right * (1.0 - mix) + runtime.hold_sample_r * mix;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Bitcrusher state.
    pub fn audit_bitcrusher(&self) -> bool {
        // SAFETY: Diagnostics are read on a non-audio control path.
        let runtime = unsafe { &*self.runtime.get() };
        let bits = Self::load(&self.bits);
        let downsample = Self::load(&self.downsample);
        bits.is_finite()
            && (1.0..=24.0).contains(&bits)
            && downsample.is_finite()
            && downsample >= 1.0
            && runtime.sample_counter.is_finite()
            && runtime.hold_sample_l.is_finite()
            && runtime.hold_sample_r.is_finite()
    }
}

#[no_mangle]
pub extern "C" fn hirari_bitcrusher_create() -> *mut c_void {
    Box::into_raw(Box::new(BitcrusherEngine::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bitcrusher_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: `state` was allocated by `hirari_bitcrusher_create`.
        unsafe { drop(Box::from_raw(state.cast::<BitcrusherEngine>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bitcrusher_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<BitcrusherEngine>().as_ref() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bitcrusher_set_bits(state: *mut c_void, bits: f32) {
    if let Some(state) = unsafe { state.cast::<BitcrusherEngine>().as_ref() } {
        state.set_bits_native(bits);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bitcrusher_set_downsample(state: *mut c_void, downsample: f32) {
    if let Some(state) = unsafe { state.cast::<BitcrusherEngine>().as_ref() } {
        state.set_downsample_native(downsample);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bitcrusher_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    mix: f32,
) {
    let Some(state) = (unsafe { state.cast::<BitcrusherEngine>().as_ref() }) else {
        return;
    };
    if left.is_null() || frames == 0 {
        return;
    }
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames as usize) };
    if right.is_null() {
        state.process_native(left, None, mix);
    } else {
        let right = unsafe { std::slice::from_raw_parts_mut(right, frames as usize) };
        state.process_native(left, Some(right), mix);
    }
}

#[cfg(test)]
mod native_behavior_tests {
    use super::BitcrusherEngine;

    #[test]
    fn native_block_matches_quantize_hold_and_wet_dry_formula() {
        let engine = BitcrusherEngine::new();
        engine.set_bits_native(3.9);
        engine.set_downsample_native(3.0);
        let mut left = [0.16, 0.4, -0.2, 0.7, -0.8, 0.3];
        let mut right = [-0.4, 0.2, 0.6, -0.7, 0.9, -0.1];
        let mut expected_left = left;
        let mut expected_right = right;
        let levels = 4.0f32;
        let mut held_left = 0.0;
        let mut held_right = 0.0;
        let mut counter = 0.0f32;
        for index in 0..left.len() {
            if counter <= 0.0 {
                held_left = (expected_left[index] * levels).round() / levels;
                held_right = (expected_right[index] * levels).round() / levels;
                counter = 3.0;
            }
            counter -= 1.0;
            expected_left[index] = expected_left[index] * 0.25 + held_left * 0.75;
            expected_right[index] = expected_right[index] * 0.25 + held_right * 0.75;
        }

        engine.process_native(&mut left, Some(&mut right), 0.75);

        assert_eq!(left, expected_left);
        assert_eq!(right, expected_right);
        assert!(engine.audit_bitcrusher());
    }

    #[test]
    fn native_mono_uses_the_left_hold_state() {
        let engine = BitcrusherEngine::new();
        engine.set_bits_native(2.0);
        engine.set_downsample_native(2.0);
        let mut mono = [0.4, 0.9, -0.6];
        engine.process_native(&mut mono, None, 1.0);
        assert_eq!(mono, [0.5, 0.5, -0.5]);
    }
}
