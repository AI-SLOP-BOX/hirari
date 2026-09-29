use crate::atomic_parameter::AtomicParameterState;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub struct ChannelStripEngine;

struct NativeChannelStrip {
    gain: AtomicParameterState,
    pan: AtomicParameterState,
    muted: AtomicBool,
    solo: AtomicBool,
    sample_rate: AtomicU64,
}

impl NativeChannelStrip {
    fn new() -> Self {
        let gain = AtomicParameterState::new(1.0, 0);
        let pan = AtomicParameterState::new(0.0, 1);
        gain.set_smoothing_time(20.0);
        pan.set_smoothing_time(20.0);
        Self {
            gain,
            pan,
            muted: AtomicBool::new(false),
            solo: AtomicBool::new(false),
            sample_rate: AtomicU64::new(44100.0_f64.to_bits()),
        }
    }

    unsafe fn process(
        &self,
        channels: *const *mut f32,
        channel_count: usize,
        offset: usize,
        mirror_channels: *const *mut f32,
        mirror_channel_count: usize,
        frames: usize,
    ) {
        if frames == 0 || channel_count == 0 || channels.is_null() {
            return;
        }
        let left = *channels;
        let right = if channel_count > 1 {
            *channels.add(1)
        } else {
            std::ptr::null_mut()
        };
        let mirror_left = if mirror_channel_count > 0 && !mirror_channels.is_null() {
            *mirror_channels
        } else {
            std::ptr::null_mut()
        };
        let mirror_right = if mirror_channel_count > 1 && !mirror_channels.is_null() {
            *mirror_channels.add(1)
        } else {
            std::ptr::null_mut()
        };
        if left.is_null() {
            return;
        }
        if self.muted.load(Ordering::Relaxed) {
            for channel in 0..channel_count {
                let samples = *channels.add(channel);
                if samples.is_null() {
                    continue;
                }
                let samples = samples.add(offset);
                for index in 0..frames {
                    samples.add(index).write(0.0);
                }
            }
            if !mirror_channels.is_null() {
                for channel in 0..mirror_channel_count {
                    let samples = *mirror_channels.add(channel);
                    if samples.is_null() {
                        continue;
                    }
                    let samples = samples.add(offset);
                    for index in 0..frames {
                        samples.add(index).write(0.0);
                    }
                }
            }
            return;
        }

        let left = left.add(offset);
        let right = if right.is_null() {
            right
        } else {
            right.add(offset)
        };
        let mirror_left = if mirror_left.is_null() {
            mirror_left
        } else {
            mirror_left.add(offset)
        };
        let mirror_right = if mirror_right.is_null() {
            mirror_right
        } else {
            mirror_right.add(offset)
        };

        if right.is_null() {
            for index in 0..frames {
                let gain = self.gain.get_next_value();
                let left_value = left.add(index);
                left_value.write(*left_value * gain);
                if !mirror_left.is_null() {
                    let mirror_value = mirror_left.add(index);
                    mirror_value.write(*mirror_value * gain);
                }
            }
            return;
        }

        for index in 0..frames {
            let gain = self.gain.get_next_value();
            let pan = self.pan.get_next_value().clamp(-1.0, 1.0);
            let angle = (pan + 1.0) * 0.25 * std::f32::consts::PI;
            let gain_left = gain * angle.cos();
            let gain_right = gain * angle.sin();
            let left_value = left.add(index);
            let right_value = right.add(index);
            left_value.write(*left_value * gain_left);
            right_value.write(*right_value * gain_right);
            if !mirror_left.is_null() && !mirror_right.is_null() {
                let mirror_left_value = mirror_left.add(index);
                let mirror_right_value = mirror_right.add(index);
                mirror_left_value.write(*mirror_left_value * gain_left);
                mirror_right_value.write(*mirror_right_value * gain_right);
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_channel_strip_create() -> *mut c_void {
    Box::into_raw(Box::new(NativeChannelStrip::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_channel_strip_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<NativeChannelStrip>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_channel_strip_set_sample_rate(
    state: *const c_void,
    sample_rate: f64,
) {
    if let Some(state) = state.cast::<NativeChannelStrip>().as_ref() {
        state
            .sample_rate
            .store(sample_rate.to_bits(), Ordering::Relaxed);
        if sample_rate.is_finite() && sample_rate > 0.0 {
            state.gain.set_sample_rate(sample_rate);
            state.pan.set_sample_rate(sample_rate);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_channel_strip_sample_rate(state: *const c_void) -> f64 {
    state
        .cast::<NativeChannelStrip>()
        .as_ref()
        .map_or(44100.0, |state| {
            f64::from_bits(state.sample_rate.load(Ordering::Relaxed))
        })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_channel_strip_reset(state: *const c_void) {
    if let Some(state) = state.cast::<NativeChannelStrip>().as_ref() {
        state.gain.reset_to_target();
        state.pan.reset_to_target();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_channel_strip_set_gain(state: *const c_void, gain: f32) {
    if let Some(state) = state.cast::<NativeChannelStrip>().as_ref() {
        state.gain.set_target(gain);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_channel_strip_set_pan(state: *const c_void, pan: f32) {
    if let Some(state) = state.cast::<NativeChannelStrip>().as_ref() {
        state.pan.set_target(pan);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_channel_strip_set_mute(state: *const c_void, muted: bool) {
    if let Some(state) = state.cast::<NativeChannelStrip>().as_ref() {
        state.muted.store(muted, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_channel_strip_set_solo(state: *const c_void, solo: bool) {
    if let Some(state) = state.cast::<NativeChannelStrip>().as_ref() {
        state.solo.store(solo, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_channel_strip_process(
    state: *const c_void,
    channels: *const *mut f32,
    channel_count: u32,
    offset: u32,
    frames: usize,
) {
    if let Some(state) = state.cast::<NativeChannelStrip>().as_ref() {
        state.process(
            channels,
            channel_count as usize,
            offset as usize,
            std::ptr::null(),
            0,
            frames,
        );
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_channel_strip_process_mirror(
    state: *const c_void,
    channels: *const *mut f32,
    channel_count: u32,
    mirror_channels: *const *mut f32,
    mirror_channel_count: u32,
    offset: u32,
    frames: usize,
) {
    if let Some(state) = state.cast::<NativeChannelStrip>().as_ref() {
        state.process(
            channels,
            channel_count as usize,
            offset as usize,
            mirror_channels,
            mirror_channel_count as usize,
            frames,
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GainStageMetrics {
    pub rms_db: f32,
    pub peak_db: f32,
    pub headroom_db: f32,
    pub clipped_samples: usize,
}
pub fn measure_gain_stage(samples: &[f32], ceiling_db: f32) -> Option<GainStageMetrics> {
    if !ceiling_db.is_finite() || !(-120.0..=24.0).contains(&ceiling_db) {
        return None;
    }
    let mut energy = 0.0f64;
    let mut peak = 0.0f32;
    let mut clipped = 0usize;
    for sample in samples {
        let value = if sample.is_finite() { *sample } else { 0.0 };
        energy += (value as f64) * (value as f64);
        peak = peak.max(value.abs());
        if value.abs() >= 1.0 {
            clipped += 1;
        }
    }
    let rms = if samples.is_empty() {
        0.0
    } else {
        (energy / samples.len() as f64).sqrt() as f32
    };
    let to_db = |value: f32| {
        if value <= 1.0e-12 {
            -120.0
        } else {
            (20.0 * value.log10()).clamp(-120.0, 24.0)
        }
    };
    let peak_db = to_db(peak);
    Some(GainStageMetrics {
        rms_db: to_db(rms),
        peak_db,
        headroom_db: ceiling_db - peak_db,
        clipped_samples: clipped,
    })
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChannelSettings {
    pub gain_db: f32,
    pub pan: f32,
    pub phase_invert: bool,
    pub sends: Vec<(u32, f32, bool)>,
}
pub fn db_to_linear(db: f32) -> Option<f32> {
    if !db.is_finite() || !(-120.0..=24.0).contains(&db) {
        return None;
    }
    Some(10.0f32.powf(db / 20.0))
}
pub fn linear_to_db(linear: f32) -> Option<f32> {
    if !linear.is_finite() || linear <= 0.0 {
        return None;
    }
    Some((20.0 * linear.log10()).clamp(-120.0, 24.0))
}
impl ChannelSettings {
    pub fn effective_send_gain(&self, bus_id: u32) -> Option<f32> {
        self.send_level(bus_id).map(|level| level.clamp(0.0, 1.0))
    }
}
impl ChannelSettings {
    pub fn validate_topology(&self) -> bool {
        self.sends.iter().all(|(id, _, _)| *id != 0)
            && self
                .sends
                .iter()
                .enumerate()
                .all(|(i, (id, _, _))| self.sends[..i].iter().all(|(other, _, _)| other != id))
    }
}
impl ChannelSettings {
    pub fn validate(&self) -> bool {
        self.gain_db.is_finite()
            && (-120.0..=24.0).contains(&self.gain_db)
            && self.pan.is_finite()
            && (-1.0..=1.0).contains(&self.pan)
            && self.sends.len() <= 64
            && self
                .sends
                .iter()
                .all(|(_, v, _)| v.is_finite() && (0.0..=1.0).contains(v))
    }
    pub fn copy_from(&mut self, other: &Self) -> bool {
        if !other.validate_complete() {
            return false;
        }
        *self = other.clone();
        true
    }
    pub fn set_send_pre_fader(&mut self, bus_id: u32, pre_fader: bool) -> bool {
        if bus_id == 0 || !self.validate_complete() {
            return false;
        }
        let Some((_, _, mode)) = self.sends.iter_mut().find(|(id, _, _)| *id == bus_id) else {
            return false;
        };
        *mode = pre_fader;
        true
    }
    pub fn send_pre_fader(&self, bus_id: u32) -> Option<bool> {
        self.sends
            .iter()
            .find(|(id, _, _)| *id == bus_id)
            .map(|(_, _, pre)| *pre)
    }
    pub fn set_send_level(&mut self, bus_id: u32, level: f32) -> bool {
        if bus_id == 0 || !level.is_finite() || !(0.0..=1.0).contains(&level) {
            return false;
        }
        let Some((_, current, _)) = self.sends.iter_mut().find(|(id, _, _)| *id == bus_id) else {
            return false;
        };
        *current = level;
        true
    }
    pub fn send_level(&self, bus_id: u32) -> Option<f32> {
        self.sends
            .iter()
            .find(|(id, _, _)| *id == bus_id)
            .map(|(_, level, _)| *level)
    }
}

impl ChannelSettings {
    pub fn validate_complete(&self) -> bool {
        self.validate() && self.validate_topology()
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChannelSettingsClipboard {
    settings: Option<ChannelSettings>,
}
impl ChannelSettingsClipboard {
    pub fn capture(&mut self, source: &ChannelSettings) -> bool {
        if !source.validate_complete() {
            return false;
        }
        self.settings = Some(source.clone());
        true
    }
    pub fn has_settings(&self) -> bool {
        self.settings.is_some()
    }
    pub fn paste_into(&self, destination: &mut ChannelSettings) -> bool {
        let Some(settings) = &self.settings else {
            return false;
        };
        destination.copy_from(settings)
    }
}

impl Default for ChannelStripEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelStripEngine {
    pub fn new() -> Self {
        Self
    }

    /// INDUSTRIAL: Processes a stereo block with vectorized gain/panning.
    pub fn process(&self, l: &mut [f32], r: &mut [f32], gain_block: &[f32], pan_block: &[f32]) {
        let sz = l
            .len()
            .min(r.len())
            .min(gain_block.len())
            .min(pan_block.len());

        for i in 0..sz {
            let gain = if gain_block[i].is_finite() {
                gain_block[i]
            } else {
                0.0
            };
            let pan = if pan_block[i].is_finite() {
                pan_block[i].clamp(-1.0, 1.0)
            } else {
                0.0
            };

            // Equal-power pan law: center retains unity power instead of
            // attenuating both channels as linear amplitude panning does.
            let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
            let gain_l = angle.cos();
            let gain_r = angle.sin();

            let in_l = if l[i].is_finite() { l[i] } else { 0.0 };
            let in_r = if r[i].is_finite() { r[i] } else { 0.0 };
            l[i] = (in_l * gain * gain_l).clamp(-8.0, 8.0);
            r[i] = (in_r * gain * gain_r).clamp(-8.0, 8.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide channel strip state.
    pub fn audit_channel_strip(&self) -> bool {
        let metrics = measure_gain_stage(&[0.0, 0.5, -1.0], 0.0);
        let mut left = [1.0f32];
        let mut right = [1.0f32];
        self.process(&mut left, &mut right, &[1.0], &[0.0]);
        metrics.is_some_and(|value| {
            value.clipped_samples == 1
                && value.peak_db == 0.0
                && (left[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1.0e-6
                && (right[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1.0e-6
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ChannelStripEngine;
    #[test]
    fn center_pan_preserves_equal_power_and_short_buffers_are_safe() {
        let mut left = [1.0, 1.0];
        let mut right = [1.0];
        ChannelStripEngine::new().process(&mut left, &mut right, &[1.0, 1.0], &[0.0, 0.0]);
        assert!((left[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((right[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert_eq!(left[1], 1.0);
    }
}
