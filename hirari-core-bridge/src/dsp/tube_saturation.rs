use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::oversampler::OversamplerEngine;

/// Thread-safe controls and allocation-free sample processing for the
/// host-facing tube saturation effect. The shaper runs through the
/// all-pass 2x oversampler (as in the former C++ path), so high-drive
/// harmonics alias far less than a base-rate tanh.
pub struct TubeSaturationEngine {
    drive: AtomicU32,
    bias: AtomicU32,
    dry_wet: AtomicU32,
    // SAFETY: each instance is processed from a single audio thread at a
    // time; the UI thread only reads the atomic controls, never this state.
    oversamplers: UnsafeCell<[OversamplerEngine; 2]>,
}

unsafe impl Sync for TubeSaturationEngine {}

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
            oversamplers: UnsafeCell::new([OversamplerEngine::new(), OversamplerEngine::new()]),
        }
    }

    pub fn reset(&self) {
        // SAFETY: reset is serialized against process by the host.
        unsafe {
            for oversampler in (*self.oversamplers.get()).iter_mut() {
                oversampler.reset();
            }
        }
    }

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

        // SAFETY: single audio thread per instance; see the field note.
        let oversamplers = unsafe { &mut *self.oversamplers.get() };
        for frame in 0..frames {
            let dry_l = if left[frame].is_finite() {
                left[frame]
            } else {
                0.0
            };
            let (up1, up2) = oversamplers[0].upsample(dry_l * gain);
            let wet_l = OversamplerEngine::downsample(
                Self::shape(up1, bias),
                Self::shape(up2, bias),
            );
            left[frame] = dry_l + mix * (wet_l - dry_l);

            if let Some(channel) = right.as_deref_mut() {
                let dry_r = if channel[frame].is_finite() {
                    channel[frame]
                } else {
                    0.0
                };
                let (up1, up2) = oversamplers[1].upsample(dry_r * gain);
                let wet_r = OversamplerEngine::downsample(
                    Self::shape(up1, bias),
                    Self::shape(up2, bias),
                );
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
pub unsafe extern "C" fn hirari_tube_saturation_save_state(
    state: *const c_void,
    bypassed: u8,
    mix: f32,
    sidechain_bus_id: u32,
    output: *mut u8,
    capacity: usize,
) -> usize {
    const STATE_SIZE: usize = 32;
    let Some(state) = state.cast::<TubeSaturationEngine>().as_ref() else {
        return 0;
    };
    if output.is_null() || capacity < STATE_SIZE || !mix.is_finite() || !(0.0..=1.0).contains(&mix)
    {
        return 0;
    }
    let mut offset = 0;
    let mut write = |field: &[u8]| {
        std::ptr::copy_nonoverlapping(field.as_ptr(), output.add(offset), field.len());
        offset += field.len();
    };
    write(&0x4155_5241u32.to_ne_bytes());
    write(&1u16.to_ne_bytes());
    write(&(u16::from(bypassed != 0)).to_ne_bytes());
    write(&mix.to_ne_bytes());
    write(&sidechain_bus_id.to_ne_bytes());
    write(&state.get_parameter(0).to_ne_bytes());
    write(&state.get_parameter(1).to_ne_bytes());
    write(&state.get_parameter(2).to_ne_bytes());
    write(&0u32.to_ne_bytes());
    STATE_SIZE
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tube_saturation_restore_state(
    state: *mut c_void,
    bytes: *const u8,
    size: usize,
    bypassed: *mut u8,
    mix: *mut f32,
    sidechain_bus_id: *mut u32,
) -> bool {
    if state.is_null()
        || bytes.is_null()
        || size != 32
        || bypassed.is_null()
        || mix.is_null()
        || sidechain_bus_id.is_null()
    {
        return false;
    }
    let read_u16 = |offset| {
        u16::from_ne_bytes(
            std::slice::from_raw_parts(bytes.add(offset), 2)
                .try_into()
                .unwrap(),
        )
    };
    let read_u32 = |offset| {
        u32::from_ne_bytes(
            std::slice::from_raw_parts(bytes.add(offset), 4)
                .try_into()
                .unwrap(),
        )
    };
    let read_f32 = |offset| {
        f32::from_ne_bytes(
            std::slice::from_raw_parts(bytes.add(offset), 4)
                .try_into()
                .unwrap(),
        )
    };
    let flags = read_u16(6);
    let decoded_mix = read_f32(8);
    let values = [read_f32(16), read_f32(20), read_f32(24)];
    if read_u32(0) != 0x4155_5241
        || read_u16(4) != 1
        || flags & !1 != 0
        || !decoded_mix.is_finite()
        || !(0.0..=1.0).contains(&decoded_mix)
        || values
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        return false;
    }
    let Some(engine) = state.cast::<TubeSaturationEngine>().as_ref() else {
        return false;
    };
    for (id, value) in values.into_iter().enumerate() {
        engine.set_parameter(id as u32, value);
    }
    *bypassed = (flags & 1) as u8;
    *mix = decoded_mix;
    *sidechain_bus_id = read_u32(12);
    true
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversampled_process_settles_to_shape_for_dc() {
        // At drive 0dB the oversampler passes DC through at unity gain
        // after settling, so the output must converge to shape(input).
        let engine = TubeSaturationEngine::new();
        let frames = 4096;
        let mut left = vec![0.5f32; frames];
        let mut right = vec![0.5f32; frames];
        engine.process(&mut left, Some(&mut right));
        let expected = TubeSaturationEngine::shape(0.5, 0.0);
        assert!(
            (left[frames - 1] - expected).abs() < 0.01,
            "settled {} vs shape {expected}",
            left[frames - 1]
        );
        assert_eq!(left[frames - 1], right[frames - 1]);
    }

    #[test]
    fn high_frequency_stays_bounded_without_zipper_noise() {
        let engine = TubeSaturationEngine::new();
        engine.set_drive(36.0);
        let frames = 8192;
        let mut left: Vec<f32> = (0..frames)
            .map(|i| 0.9 * ((2.0 * std::f32::consts::PI * 20_000.0 * i as f32) / 48_000.0).sin())
            .collect();
        let mut right = left.clone();
        engine.process(&mut left, Some(&mut right));
        let tail = &left[frames / 2..];
        assert!(tail.iter().all(|sample| sample.is_finite() && sample.abs() <= 1.0));
        // No DC offset blow-up and no NaN propagation from the clipper.
        let mean: f32 = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(mean.abs() < 0.05, "mean {mean}");
    }
}
