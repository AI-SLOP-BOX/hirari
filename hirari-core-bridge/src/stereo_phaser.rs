use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

struct PhaserRuntime {
    sample_rate: f64,
    lfo_phase: f32,
    filter_state: [[f32; 4]; 2],
    last_out: [f32; 2],
}

/// Rust-owned four-stage stereo all-pass phaser. Parameter controls are atomic
/// so host automation can update them while the audio thread renders.
pub struct StereoPhaserEngine {
    runtime: UnsafeCell<PhaserRuntime>,
    rate: AtomicU32,
    feedback: AtomicU32,
    mix: AtomicU32,
}

impl StereoPhaserEngine {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            runtime: UnsafeCell::new(PhaserRuntime {
                sample_rate: if sample_rate.is_finite() && sample_rate > 1_000.0 {
                    sample_rate
                } else {
                    44_100.0
                },
                lfo_phase: 0.0,
                filter_state: [[0.0; 4]; 2],
                last_out: [0.0; 2],
            }),
            rate: AtomicU32::new(0.5f32.to_bits()),
            feedback: AtomicU32::new(0.3f32.to_bits()),
            mix: AtomicU32::new(0.5f32.to_bits()),
        }
    }

    fn load(parameter: &AtomicU32) -> f32 {
        f32::from_bits(parameter.load(Ordering::Relaxed))
    }

    pub fn prepare(&self, sample_rate: f64) {
        // SAFETY: Host prepare/reset calls are serialized with audio processing.
        let runtime = unsafe { &mut *self.runtime.get() };
        runtime.sample_rate = if sample_rate.is_finite() && sample_rate > 1_000.0 {
            sample_rate
        } else {
            44_100.0
        };
        Self::reset_runtime(runtime);
    }

    fn reset_runtime(runtime: &mut PhaserRuntime) {
        runtime.filter_state = [[0.0; 4]; 2];
        runtime.last_out = [0.0; 2];
    }

    pub fn reset(&self) {
        // SAFETY: Host reset calls are serialized with audio processing.
        Self::reset_runtime(unsafe { &mut *self.runtime.get() });
    }

    pub fn set_rate(&self, rate: f32) {
        if rate.is_finite() {
            self.rate
                .store(rate.clamp(0.01, 20.0).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn set_feedback(&self, feedback: f32) {
        if feedback.is_finite() {
            self.feedback
                .store(feedback.clamp(-0.95, 0.95).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn set_mix(&self, mix: f32) {
        if mix.is_finite() {
            self.mix
                .store(mix.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn set_parameter(&self, id: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        match id {
            0 => self.set_rate(0.01 + value * 19.99),
            1 => self.set_feedback(-0.95 + value * 1.9),
            2 => self.set_mix(value),
            _ => {}
        }
    }

    pub fn get_parameter(&self, id: u32) -> f32 {
        match id {
            0 => ((Self::load(&self.rate) - 0.01) / 19.99).clamp(0.0, 1.0),
            1 => ((Self::load(&self.feedback) + 0.95) / 1.9).clamp(0.0, 1.0),
            2 => Self::load(&self.mix),
            _ => 0.0,
        }
    }

    pub fn process_native(&self, left: &mut [f32], mut right: Option<&mut [f32]>, mix: f32) {
        // SAFETY: The host has one audio thread per processor instance. UI-side
        // controls use atomics; prepare/reset must be serialized with process.
        let runtime = unsafe { &mut *self.runtime.get() };
        let rate = Self::load(&self.rate).clamp(0.01, 20.0);
        let feedback = Self::load(&self.feedback).clamp(-0.95, 0.95);
        let mix = if mix.is_finite() {
            mix.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let phase_increment = rate / runtime.sample_rate.max(1.0) as f32;
        let frames = right
            .as_ref()
            .map_or(left.len(), |right| left.len().min(right.len()));

        for frame in 0..frames {
            let dry_left = if left[frame].is_finite() {
                left[frame]
            } else {
                0.0
            };
            let dry_right = match right.as_ref() {
                Some(right) if right[frame].is_finite() => right[frame],
                Some(_) | None => dry_left,
            };
            let lfo_left =
                0.5f32 + 0.5f32 * (2.0f32 * std::f32::consts::PI * runtime.lfo_phase).sin();
            let lfo_right = 0.5f32
                + 0.5f32 * (2.0f32 * std::f32::consts::PI * (runtime.lfo_phase + 0.5f32)).sin();
            let coeff_left = 0.05f32 + 0.90f32 * lfo_left;
            let coeff_right = 0.05f32 + 0.90f32 * lfo_right;
            let mut wet_left = dry_left + runtime.last_out[0] * feedback;
            let mut wet_right = dry_right + runtime.last_out[1] * feedback;

            for stage in 0..4 {
                let scale = 0.85f32 + 0.04f32 * stage as f32;
                let a_left = (coeff_left * scale).clamp(-0.98, 0.98);
                let out_left = -a_left * wet_left + runtime.filter_state[0][stage];
                runtime.filter_state[0][stage] = wet_left + a_left * out_left;
                wet_left = out_left;

                let a_right = (coeff_right * scale).clamp(-0.98, 0.98);
                let out_right = -a_right * wet_right + runtime.filter_state[1][stage];
                runtime.filter_state[1][stage] = wet_right + a_right * out_right;
                wet_right = out_right;
            }

            runtime.last_out[0] = if wet_left.is_finite() { wet_left } else { 0.0 };
            runtime.last_out[1] = if wet_right.is_finite() {
                wet_right
            } else {
                0.0
            };
            left[frame] = dry_left + mix * (runtime.last_out[0] - dry_left);
            if let Some(right) = right.as_deref_mut() {
                right[frame] = dry_right + mix * (runtime.last_out[1] - dry_right);
            }
            runtime.lfo_phase += phase_increment;
            if runtime.lfo_phase >= 1.0 {
                runtime.lfo_phase -= runtime.lfo_phase.floor();
            }
        }
    }

    pub fn tail_samples(&self) -> u32 {
        // SAFETY: Tail queries happen on the control side, apart from immutable
        // sample-rate state which only changes during serialized preparation.
        let runtime = unsafe { &*self.runtime.get() };
        let sample_rate = if runtime.sample_rate.is_finite() && runtime.sample_rate > 0.0 {
            runtime.sample_rate
        } else {
            44_100.0
        };
        let feedback = Self::load(&self.feedback).abs().clamp(0.0, 0.95) as f64;
        let tail_seconds = if feedback > 0.0 {
            (0.35 * 1.0e-3f64.ln() / feedback.ln()).min(2.0)
        } else {
            0.0
        };
        (tail_seconds * sample_rate) as u32
    }

    pub fn audit_stereo_phaser(&self) -> bool {
        // SAFETY: Diagnostics are a non-audio operation.
        let runtime = unsafe { &*self.runtime.get() };
        let rate = Self::load(&self.rate);
        let feedback = Self::load(&self.feedback);
        let mix = Self::load(&self.mix);
        runtime.sample_rate.is_finite()
            && runtime.sample_rate > 1_000.0
            && runtime.lfo_phase.is_finite()
            && rate.is_finite()
            && (0.01..=20.0).contains(&rate)
            && feedback.is_finite()
            && (-0.95..=0.95).contains(&feedback)
            && mix.is_finite()
            && (0.0..=1.0).contains(&mix)
            && runtime
                .filter_state
                .iter()
                .flatten()
                .all(|value| value.is_finite())
            && runtime.last_out.iter().all(|value| value.is_finite())
    }
}

#[no_mangle]
pub extern "C" fn hirari_stereo_phaser_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(StereoPhaserEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_phaser_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: `state` was allocated by `hirari_stereo_phaser_create`.
        unsafe { drop(Box::from_raw(state.cast::<StereoPhaserEngine>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_phaser_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<StereoPhaserEngine>().as_ref() } {
        state.prepare(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_phaser_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<StereoPhaserEngine>().as_ref() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_phaser_set_parameter(
    state: *mut c_void,
    id: u32,
    value: f32,
) {
    if let Some(state) = unsafe { state.cast::<StereoPhaserEngine>().as_ref() } {
        state.set_parameter(id, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_phaser_get_parameter(state: *const c_void, id: u32) -> f32 {
    unsafe { state.cast::<StereoPhaserEngine>().as_ref() }
        .map_or(0.0, |state| state.get_parameter(id))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_phaser_set_rate(state: *mut c_void, value: f32) {
    if let Some(state) = unsafe { state.cast::<StereoPhaserEngine>().as_ref() } {
        state.set_rate(value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_phaser_set_feedback(state: *mut c_void, value: f32) {
    if let Some(state) = unsafe { state.cast::<StereoPhaserEngine>().as_ref() } {
        state.set_feedback(value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_phaser_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    mix: f32,
) {
    let Some(state) = (unsafe { state.cast::<StereoPhaserEngine>().as_ref() }) else {
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

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_phaser_tail(state: *const c_void) -> u32 {
    unsafe { state.cast::<StereoPhaserEngine>().as_ref() }
        .map_or(0, StereoPhaserEngine::tail_samples)
}

#[cfg(test)]
mod native_behavior_tests {
    use super::StereoPhaserEngine;

    #[test]
    fn stereo_process_matches_native_stage_equations() {
        let engine = StereoPhaserEngine::new(48_000.0);
        engine.set_rate(1.7);
        engine.set_feedback(-0.35);
        engine.set_mix(0.62);
        let mut left = [0.25f32, -0.5, 0.75, -1.0];
        let mut right = [-0.2f32, 0.4, -0.6, 0.8];
        let mut expected_left = left;
        let mut expected_right = right;
        let mut state = [[0.0f32; 4]; 2];
        let mut last = [0.0f32; 2];
        let mut phase = 0.0f32;
        let phase_increment = 1.7f32 / 48_000.0f32;
        for frame in 0..left.len() {
            let dry_l = expected_left[frame];
            let dry_r = expected_right[frame];
            let lfo_l = 0.5f32 + 0.5f32 * (2.0f32 * std::f32::consts::PI * phase).sin();
            let lfo_r = 0.5f32 + 0.5f32 * (2.0f32 * std::f32::consts::PI * (phase + 0.5)).sin();
            let coeff_l = 0.05f32 + 0.90f32 * lfo_l;
            let coeff_r = 0.05f32 + 0.90f32 * lfo_r;
            let mut wet_l = dry_l + last[0] * -0.35;
            let mut wet_r = dry_r + last[1] * -0.35;
            for stage in 0..4 {
                let scale = 0.85f32 + 0.04f32 * stage as f32;
                let a_l = (coeff_l * scale).clamp(-0.98, 0.98);
                let out_l = -a_l * wet_l + state[0][stage];
                state[0][stage] = wet_l + a_l * out_l;
                wet_l = out_l;
                let a_r = (coeff_r * scale).clamp(-0.98, 0.98);
                let out_r = -a_r * wet_r + state[1][stage];
                state[1][stage] = wet_r + a_r * out_r;
                wet_r = out_r;
            }
            last = [wet_l, wet_r];
            expected_left[frame] = dry_l + 0.62 * (wet_l - dry_l);
            expected_right[frame] = dry_r + 0.62 * (wet_r - dry_r);
            phase += phase_increment;
            if phase >= 1.0 {
                phase -= phase.floor();
            }
        }

        engine.process_native(&mut left, Some(&mut right), 0.62);

        assert_eq!(left, expected_left);
        assert_eq!(right, expected_right);
        assert!(engine.audit_stereo_phaser());
    }

    #[test]
    fn reset_clears_filter_memory_but_keeps_lfo_phase_like_native() {
        let engine = StereoPhaserEngine::new(44_100.0);
        let mut left = [0.4f32; 12];
        let mut right = [-0.3f32; 12];
        engine.process_native(&mut left, Some(&mut right), 1.0);
        let phase = unsafe { (*engine.runtime.get()).lfo_phase };
        engine.reset();
        assert_eq!(unsafe { (*engine.runtime.get()).lfo_phase }, phase);
        assert_eq!(
            unsafe { (*engine.runtime.get()).filter_state },
            [[0.0; 4]; 2]
        );
    }
}
