use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

struct TransientRuntime {
    sample_rate: f64,
    attack_env: f32,
    sustain_env: f32,
    attack_alpha: f32,
    sustain_alpha: f32,
    current_gain: f32,
}

pub struct TransientShaperEngine {
    attack: AtomicU32,
    sustain: AtomicU32,
    runtime: UnsafeCell<TransientRuntime>,
}

impl TransientShaperEngine {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            attack: AtomicU32::new(0.0f32.to_bits()),
            sustain: AtomicU32::new(0.0f32.to_bits()),
            runtime: UnsafeCell::new(TransientRuntime {
                sample_rate,
                attack_env: 0.0,
                sustain_env: 0.0,
                attack_alpha: 0.9,
                sustain_alpha: 0.99,
                current_gain: 1.0,
            }),
        }
    }

    pub fn set_attack(&self, attack: f32) {
        self.attack.store(attack.to_bits(), Ordering::Relaxed);
    }

    pub fn set_sustain(&self, sustain: f32) {
        self.sustain.store(sustain.to_bits(), Ordering::Relaxed);
    }

    pub fn prepare(&self, sample_rate: f64) {
        // SAFETY: host prepare is serialized with the audio callback.
        let runtime = unsafe { &mut *self.runtime.get() };
        runtime.sample_rate = sample_rate;
        runtime.attack_alpha = (-1.0 / (sample_rate * 0.005)).exp() as f32;
        runtime.sustain_alpha = (-1.0 / (sample_rate * 0.050)).exp() as f32;
    }

    pub fn reset(&self) {
        // SAFETY: host reset is serialized with the audio callback.
        let runtime = unsafe { &mut *self.runtime.get() };
        runtime.attack_env = 0.0;
        runtime.sustain_env = 0.0;
        runtime.current_gain = 1.0;
    }

    pub fn process(&self, left: &mut [f32], right: Option<&mut [f32]>) {
        // SAFETY: one audio thread owns the runtime. UI parameter writes are
        // atomic; prepare/reset are serialized with process by the host.
        let runtime = unsafe { &mut *self.runtime.get() };
        let attack = f32::from_bits(self.attack.load(Ordering::Relaxed));
        let sustain = f32::from_bits(self.sustain.load(Ordering::Relaxed));
        let attack_gain = (1.0 + attack).clamp(0.0, 2.0);
        let sustain_gain = (1.0 + sustain).clamp(0.0, 2.0);
        let mut right = right;
        let frames = right
            .as_ref()
            .map_or(left.len(), |channel| left.len().min(channel.len()));

        for frame in 0..frames {
            let input_left = left[frame];
            let input_right = right.as_ref().map_or(input_left, |channel| channel[frame]);
            let level = input_left.abs().max(input_right.abs());
            runtime.attack_env = runtime.attack_alpha * runtime.attack_env
                + (1.0 - runtime.attack_alpha) * level;
            runtime.sustain_env = runtime.sustain_alpha * runtime.sustain_env
                + (1.0 - runtime.sustain_alpha) * level;
            let transient = (runtime.attack_env - runtime.sustain_env).clamp(-1.0, 1.0);
            let body = runtime.sustain_env.clamp(0.0, 1.0);
            let gain = (1.0
                + transient * (attack_gain - 1.0)
                + body * (sustain_gain - 1.0))
            .clamp(0.0, 3.0);
            left[frame] = input_left * gain;
            if let Some(channel) = right.as_deref_mut() {
                channel[frame] = input_right * gain;
            }
        }
    }

    pub fn tail_samples(&self) -> u32 {
        0
    }
}

#[no_mangle]
pub extern "C" fn hirari_transient_shaper_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(TransientShaperEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_transient_shaper_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: handle was allocated by `hirari_transient_shaper_create`.
        unsafe { drop(Box::from_raw(state.cast::<TransientShaperEngine>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_transient_shaper_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<TransientShaperEngine>().as_ref() } {
        state.prepare(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_transient_shaper_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<TransientShaperEngine>().as_ref() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_transient_shaper_set_attack(state: *mut c_void, value: f32) {
    if let Some(state) = unsafe { state.cast::<TransientShaperEngine>().as_ref() } {
        state.set_attack(value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_transient_shaper_set_sustain(state: *mut c_void, value: f32) {
    if let Some(state) = unsafe { state.cast::<TransientShaperEngine>().as_ref() } {
        state.set_sustain(value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_transient_shaper_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
    let Some(state) = (unsafe { state.cast::<TransientShaperEngine>().as_ref() }) else {
        return;
    };
    if left.is_null() || frames == 0 || right == left {
        return;
    }
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames as usize) };
    if right.is_null() {
        state.process(left, None);
    } else {
        let right = unsafe { std::slice::from_raw_parts_mut(right, frames as usize) };
        state.process(left, Some(right));
    }
}

#[cfg(test)]
mod tests {
    use super::TransientShaperEngine;

    #[test]
    fn transient_envelopes_match_native_formula_across_split_blocks() {
        let engine = TransientShaperEngine::new(44_100.0);
        engine.prepare(48_000.0);
        engine.set_attack(0.7);
        engine.set_sustain(-0.4);
        let mut expected_left = (0..311)
            .map(|i| (i as f32 * 0.12).sin() * 0.8)
            .collect::<Vec<_>>();
        let mut expected_right = (0..311)
            .map(|i| (i as f32 * 0.17).cos() * 0.75)
            .collect::<Vec<_>>();
        let mut actual_left = expected_left.clone();
        let mut actual_right = expected_right.clone();
        let mut attack_env = 0.0f32;
        let mut sustain_env = 0.0f32;
        let attack_alpha = (-1.0 / (48_000.0f64 * 0.005)).exp() as f32;
        let sustain_alpha = (-1.0 / (48_000.0f64 * 0.050)).exp() as f32;
        for frame in 0..expected_left.len() {
            let left = expected_left[frame];
            let right = expected_right[frame];
            let level = left.abs().max(right.abs());
            attack_env = attack_alpha * attack_env + (1.0 - attack_alpha) * level;
            sustain_env = sustain_alpha * sustain_env + (1.0 - sustain_alpha) * level;
            let transient = (attack_env - sustain_env).clamp(-1.0, 1.0);
            let body = sustain_env.clamp(0.0, 1.0);
            let gain = (1.0 + transient * (1.7 - 1.0) + body * (0.6 - 1.0)).clamp(0.0, 3.0);
            expected_left[frame] = left * gain;
            expected_right[frame] = right * gain;
        }
        for range in [0..57, 57..193, 193..311] {
            engine.process(
                &mut actual_left[range.clone()],
                Some(&mut actual_right[range]),
            );
        }
        assert_eq!(actual_left, expected_left);
        assert_eq!(actual_right, expected_right);
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn rust_output_matches_frozen_cpp_processor() {
        unsafe extern "C" {
            fn hirari_transient_shaper_reference_create(sample_rate: f64) -> *mut std::ffi::c_void;
            fn hirari_transient_shaper_reference_destroy(state: *mut std::ffi::c_void);
            fn hirari_transient_shaper_reference_prepare(state: *mut std::ffi::c_void, sample_rate: f64);
            fn hirari_transient_shaper_reference_reset(state: *mut std::ffi::c_void);
            fn hirari_transient_shaper_reference_set_parameters(
                state: *mut std::ffi::c_void, attack: f32, sustain: f32);
            fn hirari_transient_shaper_reference_process(
                state: *mut std::ffi::c_void, left: *mut f32, right: *mut f32, frames: u32);
        }

        let rust = TransientShaperEngine::new(44_100.0);
        rust.prepare(48_000.0);
        rust.set_attack(0.65);
        rust.set_sustain(-0.35);
        let cpp = unsafe { hirari_transient_shaper_reference_create(44_100.0) };
        assert!(!cpp.is_null());
        unsafe {
            hirari_transient_shaper_reference_prepare(cpp, 48_000.0);
            hirari_transient_shaper_reference_set_parameters(cpp, 0.65, -0.35);
        }
        let input_left = (0..511)
            .map(|i| (i as f32 * 0.23).sin() * if i % 97 < 4 { 1.3 } else { 0.55 })
            .collect::<Vec<_>>();
        let input_right = (0..511)
            .map(|i| (i as f32 * 0.19).cos() * if i % 71 < 3 { 1.1 } else { 0.42 })
            .collect::<Vec<_>>();
        let mut rust_left = input_left.clone();
        let mut rust_right = input_right.clone();
        let mut cpp_left = input_left;
        let mut cpp_right = input_right;
        for range in [0..63, 63..256, 256..511] {
            let frames = (range.end - range.start) as u32;
            rust.process(
                &mut rust_left[range.clone()],
                Some(&mut rust_right[range.clone()]),
            );
            unsafe {
                hirari_transient_shaper_reference_process(
                    cpp,
                    cpp_left[range.clone()].as_mut_ptr(),
                    cpp_right[range].as_mut_ptr(),
                    frames,
                )
            };
        }
        for (index, (actual, expected)) in rust_left.iter().zip(&cpp_left).enumerate() {
            assert!(
                (actual - expected).abs() <= 5.0e-7,
                "left frame {index}: Rust {actual}, C++ {expected}, delta {}",
                (actual - expected).abs()
            );
        }
        for (index, (actual, expected)) in rust_right.iter().zip(&cpp_right).enumerate() {
            assert!(
                (actual - expected).abs() <= 5.0e-7,
                "right frame {index}: Rust {actual}, C++ {expected}, delta {}",
                (actual - expected).abs()
            );
        }
        rust.reset();
        unsafe { hirari_transient_shaper_reference_reset(cpp) };
        let mut reset_rust_left = [0.3f32, 0.8];
        let mut reset_rust_right = [-0.6f32, 0.2];
        let mut reset_cpp_left = reset_rust_left;
        let mut reset_cpp_right = reset_rust_right;
        rust.process(&mut reset_rust_left, Some(&mut reset_rust_right));
        unsafe {
            hirari_transient_shaper_reference_process(
                cpp,
                reset_cpp_left.as_mut_ptr(),
                reset_cpp_right.as_mut_ptr(),
                reset_cpp_left.len() as u32,
            )
        };
        assert_eq!(reset_rust_left, reset_cpp_left);
        assert_eq!(reset_rust_right, reset_cpp_right);
        unsafe { hirari_transient_shaper_reference_destroy(cpp) };
    }
}
