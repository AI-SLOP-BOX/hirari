use crate::delay_line::DelayLineEngine;
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

struct ChorusRuntime {
    sample_rate: f64,
    delay_l: DelayLineEngine,
    delay_r: DelayLineEngine,
    lfo_phase: f64,
}

pub struct StereoChorusEngine {
    runtime: UnsafeCell<ChorusRuntime>,
    rate: AtomicU32,
    mix: AtomicU32,
}

impl StereoChorusEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            runtime: UnsafeCell::new(ChorusRuntime {
                sample_rate: if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) {
                    sr
                } else {
                    44_100.0
                },
                delay_l: DelayLineEngine::new(8192),
                delay_r: DelayLineEngine::new(8192),
                lfo_phase: 0.0,
            }),
            rate: AtomicU32::new(0.8f32.to_bits()),
            mix: AtomicU32::new(0.5f32.to_bits()),
        }
    }

    fn load(parameter: &AtomicU32) -> f32 {
        f32::from_bits(parameter.load(Ordering::Relaxed))
    }

    pub fn reset(&self) {
        // SAFETY: Reset/prepare must be serialized with audio processing by the host.
        let runtime = unsafe { &mut *self.runtime.get() };
        runtime.delay_l.reset();
        runtime.delay_r.reset();
        runtime.lfo_phase = 0.0;
    }

    pub fn set_rate(&self, r: f32) {
        let value = if r.is_finite() {
            r.clamp(0.1, 5.0)
        } else {
            0.8
        };
        self.rate.store(value.to_bits(), Ordering::Relaxed);
    }

    pub fn rate(&self) -> f32 {
        Self::load(&self.rate)
    }

    pub fn set_mix(&self, m: f32) {
        let value = if m.is_finite() {
            m.clamp(0.0, 1.0)
        } else {
            0.5
        };
        self.mix.store(value.to_bits(), Ordering::Relaxed);
    }

    pub fn mix(&self) -> f32 {
        Self::load(&self.mix)
    }

    pub fn prepare(&self, sample_rate: f64) {
        // SAFETY: Reset/prepare must be serialized with audio processing by the host.
        let runtime = unsafe { &mut *self.runtime.get() };
        runtime.sample_rate =
            if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
                sample_rate
            } else {
                44_100.0
            };
        runtime.delay_l.reset();
        runtime.delay_r.reset();
        runtime.lfo_phase = 0.0;
    }

    pub fn set_parameter(&self, id: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        match id {
            0 => self.set_rate(value * 4.9 + 0.1),
            1 => self.set_mix(value),
            _ => {}
        }
    }

    pub fn get_parameter(&self, id: u32) -> f32 {
        match id {
            0 => ((self.rate() - 0.1) / 4.9).clamp(0.0, 1.0),
            1 => self.mix(),
            _ => 0.0,
        }
    }

    /// Block processing ported from the native C++ processor, preserving its
    /// integer delay taps, phase order, input guard, and output blend.
    pub fn process_native(&self, left: &mut [f32], right: Option<&mut [f32]>, mix: f32) {
        // SAFETY: There is one audio processor thread; controls are atomic and
        // host prepare/reset calls are serialized with process callbacks.
        let runtime = unsafe { &mut *self.runtime.get() };
        let sample_rate = if runtime.sample_rate.is_finite() && runtime.sample_rate >= 8_000.0 {
            runtime.sample_rate
        } else {
            44_100.0
        };
        let rate = self.rate();
        let rate = if rate.is_finite() {
            rate.clamp(0.1, 5.0)
        } else {
            0.8
        };
        let mix = if mix.is_finite() {
            mix.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let phase_inc = rate as f64 / sample_rate.max(1.0);
        if let Some(right) = right {
            let frames = left.len().min(right.len());
            for frame in 0..frames {
                let raw_left = left[frame];
                let input_left = if raw_left.is_finite() {
                    raw_left.clamp(-16.0, 16.0)
                } else {
                    0.0
                };
                let raw_right = right[frame];
                let input_right = if raw_right.is_finite() {
                    raw_right.clamp(-16.0, 16.0)
                } else {
                    input_left
                };
                let phase = (runtime.lfo_phase * 6.283_185_307) as f32;
                let delay_left = (28.0 + 12.0 * phase.sin()).clamp(1.0, 80.0) as u32;
                let delay_right =
                    (40.0 + 12.0 * (phase + 1.570_796_3).sin()).clamp(1.0, 80.0) as u32;
                let delayed_left = runtime.delay_l.process(input_left, delay_left as f32);
                let delayed_right = runtime.delay_r.process(input_right, delay_right as f32);
                left[frame] = (input_left * (1.0 - mix) + delayed_left * mix).clamp(-16.0, 16.0);
                right[frame] = (input_right * (1.0 - mix) + delayed_right * mix).clamp(-16.0, 16.0);
                Self::advance_phase(runtime, phase_inc);
            }
        } else {
            for sample in left {
                let raw = *sample;
                let input = if raw.is_finite() {
                    raw.clamp(-16.0, 16.0)
                } else {
                    0.0
                };
                let phase = (runtime.lfo_phase * 6.283_185_307) as f32;
                let delay = (28.0 + 12.0 * phase.sin()).clamp(1.0, 80.0) as u32;
                let delayed = runtime.delay_l.process(input, delay as f32);
                *sample = (input * (1.0 - mix) + delayed * mix).clamp(-16.0, 16.0);
                Self::advance_phase(runtime, phase_inc);
            }
        }
    }

    fn advance_phase(runtime: &mut ChorusRuntime, increment: f64) {
        runtime.lfo_phase += increment;
        if runtime.lfo_phase >= 1.0 {
            runtime.lfo_phase -= 1.0;
        }
    }

    /// INDUSTRIAL: Modulates delay taps to create pitch-fluctuating width.
    pub fn process(&self, l: &mut [f32], r: &mut [f32]) {
        let mix = self.mix();
        self.process_native(l, Some(r), mix);
    }

    pub fn audit_stereo_chorus(&self) -> bool {
        // SAFETY: Diagnostics are read on a non-audio control path.
        let runtime = unsafe { &*self.runtime.get() };
        let rate = self.rate();
        let mix = self.mix();
        runtime.sample_rate.is_finite()
            && runtime.sample_rate > 100.0
            && runtime.lfo_phase.is_finite()
            && rate.is_finite()
            && mix.is_finite()
            && (0.0..=1.0).contains(&mix)
            && runtime.delay_l.audit_delay_line()
            && runtime.delay_r.audit_delay_line()
    }
}

#[cfg(test)]
mod native_behavior_tests {
    use super::StereoChorusEngine;
    use crate::delay_line::DelayLineEngine;

    #[test]
    fn block_processing_matches_native_integer_taps_and_phase_order() {
        let engine = StereoChorusEngine::new(44_100.0);
        engine.set_rate(1.3);
        engine.set_mix(0.6);
        let mut left = [0.2f32, -0.4, 0.8, f32::NAN, 20.0];
        let mut right = [-0.3f32, 0.5, -0.7, 0.9, -20.0];
        let mut expected_left = left;
        let mut expected_right = right;
        let mut delay_left = DelayLineEngine::new(8192);
        let mut delay_right = DelayLineEngine::new(8192);
        let mut phase = 0.0f64;
        let dry_mix = 1.0f32 - 0.6;
        for index in 0..left.len() {
            let raw_left = expected_left[index];
            let input_left = if raw_left.is_finite() {
                raw_left.clamp(-16.0, 16.0)
            } else {
                0.0
            };
            let raw_right = expected_right[index];
            let input_right = if raw_right.is_finite() {
                raw_right.clamp(-16.0, 16.0)
            } else {
                input_left
            };
            let angle = (phase * 6.283_185_307) as f32;
            let tap_left = (28.0 + 12.0 * angle.sin()).clamp(1.0, 80.0) as u32;
            let tap_right = (40.0 + 12.0 * (angle + 1.570_796_3).sin()).clamp(1.0, 80.0) as u32;
            let wet_left = delay_left.process(input_left, tap_left as f32);
            let wet_right = delay_right.process(input_right, tap_right as f32);
            expected_left[index] = (input_left * dry_mix + wet_left * 0.6).clamp(-16.0, 16.0);
            expected_right[index] = (input_right * dry_mix + wet_right * 0.6).clamp(-16.0, 16.0);
            phase += 1.3 / 44_100.0;
            if phase >= 1.0 {
                phase -= 1.0;
            }
        }

        engine.process_native(&mut left, Some(&mut right), 0.6);

        assert_eq!(left, expected_left);
        assert_eq!(right, expected_right);
        assert!(engine.audit_stereo_chorus());
    }

    #[test]
    fn mono_processing_uses_the_left_modulation_path() {
        let engine = StereoChorusEngine::new(44_100.0);
        engine.set_mix(1.0);
        let mut mono = [1.0, 0.0, 0.0];
        engine.process_native(&mut mono, None, 1.0);
        assert_eq!(mono[0], 0.0);
        assert!(mono.iter().all(|sample| sample.is_finite()));
    }
}

#[no_mangle]
pub extern "C" fn hirari_stereo_chorus_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(StereoChorusEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_chorus_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: `state` was allocated by `hirari_stereo_chorus_create`.
        unsafe { drop(Box::from_raw(state.cast::<StereoChorusEngine>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_chorus_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<StereoChorusEngine>().as_ref() } {
        state.prepare(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_chorus_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<StereoChorusEngine>().as_ref() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_chorus_set_parameter(
    state: *mut c_void,
    id: u32,
    value: f32,
) {
    if let Some(state) = unsafe { state.cast::<StereoChorusEngine>().as_ref() } {
        state.set_parameter(id, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_chorus_get_parameter(state: *const c_void, id: u32) -> f32 {
    unsafe { state.cast::<StereoChorusEngine>().as_ref() }
        .map_or(0.0, |state| state.get_parameter(id))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_chorus_set_rate(state: *mut c_void, rate: f32) {
    if let Some(state) = unsafe { state.cast::<StereoChorusEngine>().as_ref() } {
        state.set_rate(rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_chorus_set_mix(state: *mut c_void, mix: f32) {
    if let Some(state) = unsafe { state.cast::<StereoChorusEngine>().as_ref() } {
        state.set_mix(mix);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_chorus_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    mix: f32,
) {
    let Some(state) = (unsafe { state.cast::<StereoChorusEngine>().as_ref() }) else {
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
