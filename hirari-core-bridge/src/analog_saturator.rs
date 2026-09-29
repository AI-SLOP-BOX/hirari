use crate::oversampler::OversamplerEngine;
use std::ffi::c_void;

#[derive(Clone, Copy)]
#[repr(u32)]
pub enum SaturationModel {
    Tube = 0,
    Tape = 1,
    SoftClip = 2,
}

impl SaturationModel {
    fn from_id(id: u32) -> Self {
        match id {
            1 => Self::Tape,
            2 => Self::SoftClip,
            _ => Self::Tube,
        }
    }
}

pub struct AnalogSaturatorEngine {
    pub sample_rate: f64,
    pub dc_l: f32,
    pub dc_r: f32,
    pub oversampler_l: OversamplerEngine,
    pub oversampler_r: OversamplerEngine,
}

impl AnalogSaturatorEngine {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate: if sample_rate.is_finite() && sample_rate > 1_000.0 {
                sample_rate
            } else {
                44_100.0
            },
            dc_l: 0.0,
            dc_r: 0.0,
            oversampler_l: OversamplerEngine::new(),
            oversampler_r: OversamplerEngine::new(),
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = if sample_rate.is_finite() && sample_rate > 1_000.0 {
            sample_rate
        } else {
            44_100.0
        };
    }

    pub fn reset(&mut self) {
        self.dc_l = 0.0;
        self.dc_r = 0.0;
        self.oversampler_l.reset();
        self.oversampler_r.reset();
    }

    fn apply_model(input: f32, warmth: f32, model: SaturationModel) -> f32 {
        match model {
            SaturationModel::Tube => {
                let bias = warmth * 0.25;
                (input + bias) / (1.0 + (input + bias).abs()) - bias / (1.0 + bias.abs())
            }
            SaturationModel::Tape => {
                let abs_input = input.abs();
                if abs_input < 1.0 {
                    input * (1.5 - 0.5 * input * input)
                } else if input > 0.0 {
                    1.0
                } else {
                    -1.0
                }
            }
            SaturationModel::SoftClip => input.tanh(),
        }
    }

    /// Match the production C++ processWithSettings path sample-for-sample.
    pub fn process_with_settings(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        drive: f32,
        warmth: f32,
        model: SaturationModel,
    ) {
        if !self.audit_analog_saturator() || !drive.is_finite() || !warmth.is_finite() {
            return;
        }
        let frames = left.len().min(right.len());
        let drive = drive.clamp(0.0, 4.0);
        let warmth = warmth.clamp(0.0, 1.0);
        let pre_gain = 10.0f32.powf(drive * 12.0 / 20.0);
        let post_gain = 1.0 / (pre_gain * 0.35).max(1.0);
        const DC_CUT: f32 = 0.995;

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
            let (up_l1, up_l2) = self.oversampler_l.upsample(input_l * pre_gain);
            let shaped_l1 = Self::apply_model(up_l1, warmth, model) * post_gain;
            let shaped_l2 = Self::apply_model(up_l2, warmth, model) * post_gain;
            let wet_l = OversamplerEngine::downsample(shaped_l1, shaped_l2);
            let (up_r1, up_r2) = self.oversampler_r.upsample(input_r * pre_gain);
            let shaped_r1 = Self::apply_model(up_r1, warmth, model) * post_gain;
            let shaped_r2 = Self::apply_model(up_r2, warmth, model) * post_gain;
            let wet_r = OversamplerEngine::downsample(shaped_r1, shaped_r2);

            self.dc_l = DC_CUT * self.dc_l + (1.0 - DC_CUT) * wet_l;
            self.dc_r = DC_CUT * self.dc_r + (1.0 - DC_CUT) * wet_r;
            let output_l = wet_l - self.dc_l;
            let output_r = wet_r - self.dc_r;
            left[frame] = if output_l.is_finite() { output_l } else { 0.0 };
            right[frame] = if output_r.is_finite() { output_r } else { 0.0 };
        }
    }

    pub fn audit_analog_saturator(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 1_000.0
            && self.dc_l.is_finite()
            && self.dc_r.is_finite()
            && self.oversampler_l.phase_state_1.is_finite()
            && self.oversampler_l.phase_state_2.is_finite()
            && self.oversampler_r.phase_state_1.is_finite()
            && self.oversampler_r.phase_state_2.is_finite()
    }
}

#[no_mangle]
pub extern "C" fn hirari_analog_saturator_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(AnalogSaturatorEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_analog_saturator_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: The pointer is created by hirari_analog_saturator_create and destroyed once.
        drop(Box::from_raw(state.cast::<AnalogSaturatorEngine>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_analog_saturator_set_sample_rate(
    state: *mut c_void,
    sample_rate: f64,
) {
    if let Some(state) = state.cast::<AnalogSaturatorEngine>().as_mut() {
        state.set_sample_rate(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_analog_saturator_reset(state: *mut c_void) {
    if let Some(state) = state.cast::<AnalogSaturatorEngine>().as_mut() {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_analog_saturator_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    drive: f32,
    warmth: f32,
    model: u32,
) {
    let Some(state) = state.cast::<AnalogSaturatorEngine>().as_mut() else {
        return;
    };
    if left.is_null() || right.is_null() || frames == 0 {
        return;
    }
    // The native processor permits mono by passing the same pointer for both channels.
    // Copy into bounded vectors would allocate on the audio thread, so process the
    // aliased case inline with the same read-before-write ordering as the old C++.
    if left == right {
        if !drive.is_finite() || !warmth.is_finite() || !state.audit_analog_saturator() {
            return;
        }
        let drive = drive.clamp(0.0, 4.0);
        let warmth = warmth.clamp(0.0, 1.0);
        let pre_gain = 10.0f32.powf(drive * 12.0 / 20.0);
        let post_gain = 1.0 / (pre_gain * 0.35).max(1.0);
        const DC_CUT: f32 = 0.995;
        for frame in 0..frames as usize {
            let input = unsafe { *left.add(frame) };
            let input_l = if input.is_finite() { input } else { 0.0 };
            let input_r = input_l;
            let (l1, l2) = state.oversampler_l.upsample(input_l * pre_gain);
            let wet_l = OversamplerEngine::downsample(
                AnalogSaturatorEngine::apply_model(l1, warmth, SaturationModel::from_id(model))
                    * post_gain,
                AnalogSaturatorEngine::apply_model(l2, warmth, SaturationModel::from_id(model))
                    * post_gain,
            );
            let (r1, r2) = state.oversampler_r.upsample(input_r * pre_gain);
            let wet_r = OversamplerEngine::downsample(
                AnalogSaturatorEngine::apply_model(r1, warmth, SaturationModel::from_id(model))
                    * post_gain,
                AnalogSaturatorEngine::apply_model(r2, warmth, SaturationModel::from_id(model))
                    * post_gain,
            );
            state.dc_l = DC_CUT * state.dc_l + (1.0 - DC_CUT) * wet_l;
            state.dc_r = DC_CUT * state.dc_r + (1.0 - DC_CUT) * wet_r;
            let output_l = wet_l - state.dc_l;
            let output_r = wet_r - state.dc_r;
            unsafe {
                *left.add(frame) = if output_l.is_finite() { output_l } else { 0.0 };
            }
            unsafe {
                *right.add(frame) = if output_r.is_finite() { output_r } else { 0.0 };
            }
        }
        return;
    }
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames as usize) };
    let right = unsafe { std::slice::from_raw_parts_mut(right, frames as usize) };
    state.process_with_settings(left, right, drive, warmth, SaturationModel::from_id(model));
}

#[cfg(test)]
mod tests {
    use super::{AnalogSaturatorEngine, SaturationModel};

    unsafe extern "C" {
        fn hirari_analog_saturator_frozen_reference(
            input_left: *const f32,
            input_right: *const f32,
            output_left: *mut f32,
            output_right: *mut f32,
            frames: u32,
            drive: f32,
            warmth: f32,
            model: u32,
        );
    }

    #[test]
    fn three_models_process_stereo_blocks_and_reset_state() {
        for model in [
            SaturationModel::Tube,
            SaturationModel::Tape,
            SaturationModel::SoftClip,
        ] {
            let mut engine = AnalogSaturatorEngine::new(48_000.0);
            let mut left = [0.0, 0.1, -0.2, 0.8, f32::NAN, 0.3];
            let mut right = [0.0, -0.15, 0.25, -0.7, f32::INFINITY, -0.4];
            engine.process_with_settings(&mut left, &mut right, 1.2, 0.4, model);
            assert!(left
                .iter()
                .chain(right.iter())
                .all(|sample| sample.is_finite()));
            assert!(engine.audit_analog_saturator());
            engine.reset();
            assert_eq!(engine.dc_l, 0.0);
            assert_eq!(engine.dc_r, 0.0);
        }
    }

    #[test]
    fn rust_saturator_matches_frozen_cpp_for_audio_fixtures_and_all_models() {
        let input_left = [0.0, 0.125, -0.75, 1.5, f32::NAN, 0.4, -0.2, f32::INFINITY];
        let input_right = [0.0, -0.25, 0.5, -1.25, f32::NEG_INFINITY, 0.3, -0.8, 0.05];
        for (model_id, model) in [
            SaturationModel::Tube,
            SaturationModel::Tape,
            SaturationModel::SoftClip,
        ]
        .into_iter()
        .enumerate()
        {
            let mut expected_left = [0.0; 8];
            let mut expected_right = [0.0; 8];
            // SAFETY: All input/output arrays contain eight elements and are disjoint.
            unsafe {
                hirari_analog_saturator_frozen_reference(
                    input_left.as_ptr(),
                    input_right.as_ptr(),
                    expected_left.as_mut_ptr(),
                    expected_right.as_mut_ptr(),
                    8,
                    1.35,
                    0.62,
                    model_id as u32,
                );
            }
            let mut actual_left = input_left;
            let mut actual_right = input_right;
            let mut engine = AnalogSaturatorEngine::new(48_000.0);
            engine.process_with_settings(&mut actual_left, &mut actual_right, 1.35, 0.62, model);
            for (index, (expected, actual)) in expected_left.iter().zip(actual_left).enumerate() {
                assert!(
                    (expected - actual).abs() <= 2.0e-6,
                    "left model={model_id}, frame={index}, expected={expected}, actual={actual}"
                );
            }
            for (index, (expected, actual)) in expected_right.iter().zip(actual_right).enumerate() {
                assert!(
                    (expected - actual).abs() <= 2.0e-6,
                    "right model={model_id}, frame={index}, expected={expected}, actual={actual}"
                );
            }
        }
    }
}
