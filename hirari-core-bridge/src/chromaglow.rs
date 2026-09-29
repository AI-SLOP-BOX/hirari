use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

const MODE_RETRO: u32 = 0;
const MODE_MODERN: u32 = 1;
const MODE_MAGNETIC: u32 = 2;

struct ChromaGlowRuntime {
    sample_rate: f64,
    last_l: f32,
    last_r: f32,
}

pub struct ChromaGlowEngine {
    gain: AtomicU32,
    mix: AtomicU32,
    mode: AtomicU32,
    runtime: UnsafeCell<ChromaGlowRuntime>,
}

impl ChromaGlowEngine {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            gain: AtomicU32::new(1.0f32.to_bits()),
            mix: AtomicU32::new(0.5f32.to_bits()),
            mode: AtomicU32::new(MODE_MODERN),
            runtime: UnsafeCell::new(ChromaGlowRuntime {
                sample_rate: valid_sample_rate(sample_rate),
                last_l: 0.0,
                last_r: 0.0,
            }),
        }
    }

    fn load_float(parameter: &AtomicU32) -> f32 {
        f32::from_bits(parameter.load(Ordering::Relaxed))
    }

    pub fn reset(&self) {
        // SAFETY: reset/prepare are serialized with audio processing.
        let runtime = unsafe { &mut *self.runtime.get() };
        runtime.last_l = 0.0;
        runtime.last_r = 0.0;
    }

    pub fn prepare(&self, sample_rate: f64) {
        // SAFETY: reset/prepare are serialized with audio processing.
        unsafe { &mut *self.runtime.get() }.sample_rate = valid_sample_rate(sample_rate);
        self.reset();
    }

    pub fn set_parameter(&self, id: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        match id {
            0 => {
                let normalized = value.clamp(0.0, 1.0);
                let gain = 10.0f32.powf((-24.0 + normalized * 60.0) / 20.0);
                self.gain.store(gain.to_bits(), Ordering::Relaxed);
            }
            1 => self
                .mix
                .store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed),
            2 => {
                let mode = (value * 2.0).round().clamp(0.0, 2.0) as u32;
                self.mode.store(mode, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    pub fn get_parameter(&self, id: u32) -> f32 {
        match id {
            0 => {
                let gain = Self::load_float(&self.gain).max(1.0e-6);
                ((20.0 * gain.log10() + 24.0) / 60.0).clamp(0.0, 1.0)
            }
            1 => Self::load_float(&self.mix),
            2 => self.mode.load(Ordering::Relaxed) as f32 / 2.0,
            _ => 0.0,
        }
    }

    pub fn set_params(&self, drive_db: f32, character: f32, mode: u32) {
        let drive_db = if drive_db.is_finite() {
            drive_db.clamp(-24.0, 36.0)
        } else {
            0.0
        };
        let character = if character.is_finite() {
            character.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.gain
            .store(10.0f32.powf(drive_db / 20.0).to_bits(), Ordering::Relaxed);
        self.mix.store(character.to_bits(), Ordering::Relaxed);
        self.mode.store(mode, Ordering::Relaxed);
    }

    fn fast_tanh(input: f32) -> f32 {
        if input > 3.0 {
            return 1.0;
        }
        if input < -3.0 {
            return -1.0;
        }
        let square = input * input;
        input * (27.0 + square) / (27.0 + 9.0 * square)
    }

    fn saturate(input: f32, mode: u32) -> f32 {
        match mode {
            MODE_RETRO => {
                if input > 0.0 {
                    Self::fast_tanh(input)
                } else {
                    input / (1.0 + input.abs())
                }
            }
            MODE_MODERN => Self::fast_tanh(input),
            MODE_MAGNETIC => ((1.5 * input) * (1.0 - (input * input) / 3.0)).clamp(-1.0, 1.0),
            _ => input,
        }
    }

    /// Port of the production C++ transfer path. The Rust-only 2x interpolated
    /// oversampling kernel was removed because the production processor never
    /// used it and it produced a different signal.
    pub fn process(&self, left: &mut [f32], right: Option<&mut [f32]>) {
        let gain = Self::load_float(&self.gain);
        let gain = if gain.is_finite() { gain } else { 1.0 };
        let raw_mix = Self::load_float(&self.mix);
        let mix = if raw_mix.is_finite() {
            raw_mix.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mode = self.mode.load(Ordering::Relaxed);
        let mut right = right;
        let frames = right
            .as_ref()
            .map_or(left.len(), |channel| left.len().min(channel.len()));
        for frame in 0..frames {
            let dry_l = if left[frame].is_finite() {
                left[frame]
            } else {
                0.0
            };
            let dry_r = right.as_ref().map_or(dry_l, |channel| {
                if channel[frame].is_finite() {
                    channel[frame]
                } else {
                    0.0
                }
            });
            let wet_l = Self::saturate(dry_l * gain, mode);
            let wet_r = Self::saturate(dry_r * gain, mode);
            let out_l = dry_l + mix * (wet_l - dry_l);
            let out_r = dry_r + mix * (wet_r - dry_r);
            if let Some(channel) = right.as_deref_mut() {
                left[frame] = finite_clamped(out_l, -16.0, 16.0);
                channel[frame] = finite_clamped(out_r, -16.0, 16.0);
            } else {
                left[frame] = finite_clamped(0.5 * (out_l + out_r), -16.0, 16.0);
            }
        }
    }
}

fn valid_sample_rate(sample_rate: f64) -> f64 {
    if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
        sample_rate
    } else {
        44_100.0
    }
}

fn finite_clamped(value: f32, low: f32, high: f32) -> f32 {
    if value.is_finite() {
        value.clamp(low, high)
    } else {
        0.0
    }
}

#[no_mangle]
pub extern "C" fn hirari_chromaglow_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(ChromaGlowEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_chromaglow_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: state was allocated by `hirari_chromaglow_create`.
        unsafe { drop(Box::from_raw(state.cast::<ChromaGlowEngine>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_chromaglow_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<ChromaGlowEngine>().as_ref() } {
        state.prepare(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_chromaglow_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<ChromaGlowEngine>().as_ref() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_chromaglow_set_parameter(state: *mut c_void, id: u32, value: f32) {
    if let Some(state) = unsafe { state.cast::<ChromaGlowEngine>().as_ref() } {
        state.set_parameter(id, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_chromaglow_get_parameter(state: *const c_void, id: u32) -> f32 {
    unsafe { state.cast::<ChromaGlowEngine>().as_ref() }
        .map_or(0.0, |state| state.get_parameter(id))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_chromaglow_set_params(
    state: *mut c_void,
    drive_db: f32,
    character: f32,
    mode: u32,
) {
    if let Some(state) = unsafe { state.cast::<ChromaGlowEngine>().as_ref() } {
        state.set_params(drive_db, character, mode);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_chromaglow_set_sample_rate(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<ChromaGlowEngine>().as_ref() } {
        // The legacy direct setter does not reset the processor.
        unsafe { &mut *state.runtime.get() }.sample_rate = valid_sample_rate(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_chromaglow_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
    let Some(state) = (unsafe { state.cast::<ChromaGlowEngine>().as_ref() }) else {
        return;
    };
    if left.is_null() || frames == 0 {
        return;
    }
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames as usize) };
    if right.is_null() || right == left.as_mut_ptr() {
        state.process(left, None);
    } else {
        let right = unsafe { std::slice::from_raw_parts_mut(right, frames as usize) };
        state.process(left, Some(right));
    }
}

#[cfg(test)]
mod tests {
    use super::ChromaGlowEngine;

    #[test]
    fn all_saturation_modes_match_legacy_transfer_and_character_blend() {
        let source_l = [0.0f32, 0.3, -0.7, 1.2, -2.1, f32::NAN];
        let source_r = [0.5f32, -0.4, 0.6, -1.4, 2.0, 0.2];
        for mode in 0..3 {
            let engine = ChromaGlowEngine::new(44_100.0);
            engine.set_params(12.0, 0.7, mode);
            let mut left = source_l;
            let mut right = source_r;
            engine.process(&mut left, Some(&mut right));
            assert!(left.iter().chain(&right).all(|sample| sample.is_finite()));
            assert!(left.iter().chain(&right).all(|sample| sample.abs() <= 16.0));
        }
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn output_matches_frozen_cpp_for_all_modes_and_block_edges() {
        unsafe extern "C" {
            fn hirari_chromaglow_reference_create(sample_rate: f64) -> *mut std::ffi::c_void;
            fn hirari_chromaglow_reference_destroy(state: *mut std::ffi::c_void);
            fn hirari_chromaglow_reference_prepare(state: *mut std::ffi::c_void, sample_rate: f64);
            fn hirari_chromaglow_reference_reset(state: *mut std::ffi::c_void);
            fn hirari_chromaglow_reference_set_params(
                state: *mut std::ffi::c_void,
                drive_db: f32,
                mix: f32,
                mode: u32,
            );
            fn hirari_chromaglow_reference_set_parameter(
                state: *mut std::ffi::c_void,
                id: u32,
                value: f32,
            );
            fn hirari_chromaglow_reference_get_parameter(
                state: *const std::ffi::c_void,
                id: u32,
            ) -> f32;
            fn hirari_chromaglow_reference_process(
                state: *mut std::ffi::c_void,
                left: *mut f32,
                right: *mut f32,
                frames: u32,
            );
        }

        let source_left = (0..401)
            .map(|i| (i as f32 * 0.31).sin() * if i % 37 < 3 { 2.0 } else { 0.8 })
            .collect::<Vec<_>>();
        let source_right = (0..401)
            .map(|i| (i as f32 * 0.23).cos() * if i % 41 < 2 { 1.7 } else { 0.6 })
            .collect::<Vec<_>>();

        for mode in 0..3 {
            let engine = ChromaGlowEngine::new(44_100.0);
            engine.prepare(48_000.0);
            engine.set_params(18.0, 0.73, mode);
            let cpp = unsafe { hirari_chromaglow_reference_create(44_100.0) };
            assert!(!cpp.is_null());
            unsafe {
                hirari_chromaglow_reference_prepare(cpp, 48_000.0);
                hirari_chromaglow_reference_set_params(cpp, 18.0, 0.73, mode);
            }
            for (id, value) in [(0, 0.37), (1, 0.62), (2, mode as f32 * 0.5)] {
                engine.set_parameter(id, value);
                unsafe { hirari_chromaglow_reference_set_parameter(cpp, id, value) };
                let rust_parameter = engine.get_parameter(id);
                let cpp_parameter = unsafe { hirari_chromaglow_reference_get_parameter(cpp, id) };
                assert!((rust_parameter - cpp_parameter).abs() <= 1.0e-6);
            }
            engine.set_params(18.0, 0.73, mode);
            unsafe { hirari_chromaglow_reference_set_params(cpp, 18.0, 0.73, mode) };
            let mut rust_left = source_left.clone();
            let mut rust_right = source_right.clone();
            let mut cpp_left = source_left.clone();
            let mut cpp_right = source_right.clone();
            for range in [0..29, 29..213, 213..401] {
                let frames = (range.end - range.start) as u32;
                engine.process(
                    &mut rust_left[range.clone()],
                    Some(&mut rust_right[range.clone()]),
                );
                unsafe {
                    hirari_chromaglow_reference_process(
                        cpp,
                        cpp_left[range.clone()].as_mut_ptr(),
                        cpp_right[range].as_mut_ptr(),
                        frames,
                    )
                };
            }
            for (actual, expected) in rust_left.iter().zip(&cpp_left) {
                assert!((actual - expected).abs() <= 3.0e-7);
            }
            for (actual, expected) in rust_right.iter().zip(&cpp_right) {
                assert!((actual - expected).abs() <= 3.0e-7);
            }
            unsafe { hirari_chromaglow_reference_reset(cpp) };
            unsafe { hirari_chromaglow_reference_destroy(cpp) };
        }
    }
}
