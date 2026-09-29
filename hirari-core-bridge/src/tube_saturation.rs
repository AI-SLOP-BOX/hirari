use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

/// Thread-safe controls and allocation-free sample processing for the
/// host-facing tube saturation effect.
pub struct TubeSaturationEngine {
    drive: AtomicU32,
    bias: AtomicU32,
    dry_wet: AtomicU32,
}

impl Default for TubeSaturationEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TubeSaturationEngine {
    pub fn new() -> Self {
        Self {
            drive: AtomicU32::new(0.0_f32.to_bits()),
            bias: AtomicU32::new(0.0_f32.to_bits()),
            dry_wet: AtomicU32::new(1.0_f32.to_bits()),
        }
    }

    pub fn reset(&self) {}

    fn load(parameter: &AtomicU32) -> f32 {
        f32::from_bits(parameter.load(Ordering::Relaxed))
    }

    pub fn drive(&self) -> f32 {
        Self::load(&self.drive)
    }

    pub fn bias(&self) -> f32 {
        Self::load(&self.bias)
    }

    pub fn dry_wet(&self) -> f32 {
        Self::load(&self.dry_wet)
    }

    pub fn set_drive(&self, db: f32) {
        if db.is_finite() {
            self.drive
                .store(db.clamp(-24.0, 36.0).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn set_bias(&self, bias: f32) {
        if bias.is_finite() {
            self.bias
                .store(bias.clamp(-1.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn set_dry_wet(&self, mix: f32) {
        if mix.is_finite() {
            self.dry_wet
                .store(mix.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn set_parameter(&self, id: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        let normalized = value.clamp(0.0, 1.0);
        match id {
            0 => self.set_drive(-24.0 + normalized * 60.0),
            1 => self.set_bias(-1.0 + normalized * 2.0),
            2 => self.set_dry_wet(value),
            _ => {}
        }
    }

    pub fn get_parameter(&self, id: u32) -> f32 {
        match id {
            0 => ((self.drive() + 24.0) / 60.0).clamp(0.0, 1.0),
            1 => ((self.bias() + 1.0) * 0.5).clamp(0.0, 1.0),
            2 => self.dry_wet(),
            _ => 0.0,
        }
    }

    pub fn process(&self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        let frames = right
            .as_ref()
            .map_or(left.len(), |channel| left.len().min(channel.len()));
        let drive = self.drive().clamp(-24.0, 36.0);
        let bias = self.bias().clamp(-1.0, 1.0);
        let mix = self.dry_wet().clamp(0.0, 1.0);
        let gain = 10.0_f32.powf(drive / 20.0);

        for frame in 0..frames {
            let dry_l = if left[frame].is_finite() {
                left[frame]
            } else {
                0.0
            };
            let wet_l = Self::shape(dry_l * gain, bias);
            left[frame] = dry_l + mix * (wet_l - dry_l);

            if let Some(channel) = right.as_deref_mut() {
                let dry_r = if channel[frame].is_finite() {
                    channel[frame]
                } else {
                    0.0
                };
                let wet_r = Self::shape(dry_r * gain, bias);
                channel[frame] = dry_r + mix * (wet_r - dry_r);
            }
        }
    }

    #[inline]
    fn shape(input: f32, bias: f32) -> f32 {
        let x = (input + bias * 0.15).clamp(-8.0, 8.0);
        let wet = x.tanh() + 0.08 * (x * 2.0).tanh() * (1.0 + bias);
        (wet * 0.88 - bias * 0.04).clamp(-1.0, 1.0)
    }
}

#[no_mangle]
pub extern "C" fn hirari_tube_saturation_create() -> *mut c_void {
    Box::into_raw(Box::new(TubeSaturationEngine::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tube_saturation_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<TubeSaturationEngine>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tube_saturation_reset(state: *mut c_void) {
    if let Some(state) = state.cast::<TubeSaturationEngine>().as_ref() {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tube_saturation_set_parameter(
    state: *const c_void,
    id: u32,
    value: f32,
) {
    if let Some(state) = state.cast::<TubeSaturationEngine>().as_ref() {
        state.set_parameter(id, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tube_saturation_get_parameter(
    state: *const c_void,
    id: u32,
) -> f32 {
    state
        .cast::<TubeSaturationEngine>()
        .as_ref()
        .map_or(0.0, |state| state.get_parameter(id))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tube_saturation_set_control(
    state: *const c_void,
    control: u32,
    value: f32,
) {
    let Some(state) = state.cast::<TubeSaturationEngine>().as_ref() else {
        return;
    };
    match control {
        0 => state.set_drive(value),
        1 => state.set_bias(value),
        2 => state.set_dry_wet(value),
        _ => {}
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tube_saturation_process(
    state: *const c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) {
    if left.is_null() {
        return;
    }
    let Some(state) = state.cast::<TubeSaturationEngine>().as_ref() else {
        return;
    };
    let left = std::slice::from_raw_parts_mut(left, frames);
    if right.is_null() {
        state.process(left, None);
    } else {
        state.process(left, Some(std::slice::from_raw_parts_mut(right, frames)));
    }
}
