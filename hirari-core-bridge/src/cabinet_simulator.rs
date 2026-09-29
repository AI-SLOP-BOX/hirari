use std::ffi::c_void;

const TAPS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum CabinetModel {
    Generic = 0,
    Stack4x12 = 1,
    Combo1x12 = 2,
}

pub struct CabinetSimulatorEngine {
    fir: [f32; TAPS],
    history: [[f32; TAPS]; 2],
    write_idx: usize,
}

impl Default for CabinetSimulatorEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl CabinetSimulatorEngine {
    pub fn new() -> Self {
        let mut engine = Self {
            fir: [0.0; TAPS],
            history: [[0.0; TAPS]; 2],
            write_idx: 0,
        };
        engine.set_model(CabinetModel::Stack4x12);
        engine
    }

    pub fn reset(&mut self) {
        self.history = [[0.0; TAPS]; 2];
        self.write_idx = 0;
    }

    pub fn set_model(&mut self, model: CabinetModel) {
        self.fir.fill(0.0);
        let (decay, resonance, secondary, phase, reflection_start, transient) = match model {
            CabinetModel::Generic => {
                self.fir[0] = 1.0;
                return;
            }
            CabinetModel::Stack4x12 => (0.060, 0.31, 0.16, 0.15, 17usize, 0.42),
            CabinetModel::Combo1x12 => (0.095, 0.52, 0.10, 0.42, 11usize, 0.58),
        };

        for index in 0..TAPS {
            let t = index as f32;
            let envelope = (-decay * t).exp();
            let body = (resonance * t + phase).sin();
            let reflection = if index >= reflection_start {
                secondary * (-0.11 * (index - reflection_start) as f32).exp() * (0.19 * t).cos()
            } else {
                0.0
            };
            self.fir[index] = envelope * (0.82 * body + reflection);
        }
        self.fir[0] += transient;

        let energy: f32 = self.fir.iter().map(|tap| tap * tap).sum();
        if energy > 1.0e-9 && energy.is_finite() {
            let scale = 0.92 / energy.sqrt();
            for tap in &mut self.fir {
                *tap *= scale;
            }
        }
    }

    pub fn process(&mut self, left: &mut [f32], right: Option<&mut [f32]>) {
        let len = right
            .as_ref()
            .map_or(left.len(), |r| left.len().min(r.len()));
        let mut right = right;
        for index in 0..len {
            let input_left = if left[index].is_finite() {
                left[index]
            } else {
                0.0
            };
            let input_right = right.as_ref().map_or(input_left, |r| {
                if r[index].is_finite() {
                    r[index]
                } else {
                    input_left
                }
            });
            self.history[0][self.write_idx] = input_left;
            self.history[1][self.write_idx] = input_right;

            let mut output_left = 0.0;
            let mut output_right = 0.0;
            for tap in 0..TAPS {
                let history_index = (self.write_idx + TAPS - tap) % TAPS;
                output_left += self.history[0][history_index] * self.fir[tap];
                output_right += self.history[1][history_index] * self.fir[tap];
            }
            left[index] = if output_left.is_finite() {
                output_left.clamp(-4.0, 4.0)
            } else {
                0.0
            };
            if let Some(right) = right.as_deref_mut() {
                right[index] = if output_right.is_finite() {
                    output_right.clamp(-4.0, 4.0)
                } else {
                    0.0
                };
            }
            self.write_idx = (self.write_idx + 1) % TAPS;
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_cabinet_simulator_create() -> *mut c_void {
    Box::into_raw(Box::new(CabinetSimulatorEngine::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_cabinet_simulator_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: the handle was allocated by `hirari_cabinet_simulator_create`.
        drop(unsafe { Box::from_raw(state.cast::<CabinetSimulatorEngine>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_cabinet_simulator_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<CabinetSimulatorEngine>().as_mut() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_cabinet_simulator_set_model(state: *mut c_void, model: u32) {
    let Some(state) = (unsafe { state.cast::<CabinetSimulatorEngine>().as_mut() }) else {
        return;
    };
    let model = match model {
        0 => CabinetModel::Generic,
        1 => CabinetModel::Stack4x12,
        2 => CabinetModel::Combo1x12,
        _ => return,
    };
    state.set_model(model);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_cabinet_simulator_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
    if state.is_null() || left.is_null() || frames == 0 {
        return;
    }
    // SAFETY: caller supplies `frames` readable/writable samples per plane.
    let engine = unsafe { &mut *state.cast::<CabinetSimulatorEngine>() };
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames as usize) };
    if right.is_null() || right == left.as_mut_ptr() {
        engine.process(left, None);
    } else {
        let right = unsafe { std::slice::from_raw_parts_mut(right, frames as usize) };
        engine.process(left, Some(right));
    }
}

#[cfg(test)]
mod tests {
    use super::{CabinetModel, CabinetSimulatorEngine, TAPS};

    #[test]
    fn cabinet_models_are_normalized_and_distinct() {
        let mut engine = CabinetSimulatorEngine::new();
        let stack = engine.fir;
        let stack_energy: f32 = stack.iter().map(|tap| tap * tap).sum();
        assert!((stack_energy.sqrt() - 0.92).abs() < 1.0e-5);

        engine.set_model(CabinetModel::Combo1x12);
        assert_ne!(engine.fir, stack);
        let combo_energy: f32 = engine.fir.iter().map(|tap| tap * tap).sum();
        assert!((combo_energy.sqrt() - 0.92).abs() < 1.0e-5);

        engine.set_model(CabinetModel::Generic);
        assert_eq!(engine.fir[0], 1.0);
        assert!(engine.fir[1..].iter().all(|tap| *tap == 0.0));
    }

    #[test]
    fn processing_is_streaming_finite_stereo_and_mono_safe() {
        let mut engine = CabinetSimulatorEngine::new();
        assert_eq!(engine.fir.len(), TAPS);
        let mut left = [0.0; 160];
        let mut right = [0.0; 160];
        left[0] = 1.0;
        right[0] = f32::NAN;
        engine.process(&mut left[..64], Some(&mut right[..64]));
        engine.process(&mut left[64..], Some(&mut right[64..]));
        assert!(left
            .iter()
            .all(|sample| sample.is_finite() && sample.abs() <= 4.0));
        assert!(right
            .iter()
            .all(|sample| sample.is_finite() && sample.abs() <= 4.0));

        engine.reset();
        let mut mono = [0.0; 8];
        mono[0] = 0.5;
        engine.process(&mut mono, None);
        assert!(mono.iter().all(|sample| sample.is_finite()));
        assert_ne!(mono[0], 0.5);
    }
}
