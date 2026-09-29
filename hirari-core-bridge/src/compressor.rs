use std::ffi::c_void;

const MAX_LOOKAHEAD: usize = 4096;

/// Stateful mastering compressor. The delay workspace is prepared before the
/// audio callback and the processing path does not allocate.
pub struct DynamicCompressorEngine {
    pub sample_rate: f64,
    pub threshold_db: f32,
    pub ratio: f32,
    pub makeup_db: f32,
    pub knee_db: f32,
    pub auto_gain: bool,
    pub use_rms: bool,
    pub rms_sum: f32,
    pub attack_alpha: f32,
    pub release_alpha: f32,
    pub envelope: f32,
    pub current_gr: f32,
    pub delay_l: Vec<f32>,
    pub delay_r: Vec<f32>,
    pub write_idx: usize,
    pub lookahead_samples: usize,
}

impl DynamicCompressorEngine {
    pub fn new(sample_rate: f64) -> Self {
        let mut engine = Self {
            sample_rate: 44_100.0,
            threshold_db: -20.0,
            ratio: 4.0,
            makeup_db: 0.0,
            knee_db: 6.0,
            auto_gain: true,
            use_rms: false,
            rms_sum: 0.0,
            attack_alpha: 0.9,
            release_alpha: 0.999,
            envelope: 0.0,
            current_gr: 1.0,
            delay_l: vec![0.0; MAX_LOOKAHEAD],
            delay_r: vec![0.0; MAX_LOOKAHEAD],
            write_idx: 0,
            lookahead_samples: 0,
        };
        engine.prepare_to_play(sample_rate);
        engine
    }

    pub fn prepare_to_play(&mut self, sample_rate: f64) {
        self.sample_rate =
            if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
                sample_rate
            } else {
                44_100.0
            };
        self.delay_l.fill(0.0);
        self.delay_r.fill(0.0);
        self.write_idx = 0;
    }

    pub fn reset(&mut self) {
        self.envelope = 0.0;
        self.current_gr = 1.0;
        self.rms_sum = 0.0;
        self.delay_l.fill(0.0);
        self.delay_r.fill(0.0);
        self.write_idx = 0;
    }

    pub fn set_lookahead(&mut self, milliseconds: f32) {
        let milliseconds = if milliseconds.is_nan() {
            0.0
        } else {
            milliseconds.max(0.0)
        };
        self.lookahead_samples =
            ((milliseconds * self.sample_rate as f32 * 0.001) as usize).min(MAX_LOOKAHEAD - 1);
    }

    pub fn set_threshold(&mut self, db: f32) {
        self.threshold_db = if db.is_nan() {
            -100.0
        } else {
            db.clamp(-100.0, 0.0)
        };
    }

    pub fn set_ratio(&mut self, ratio: f32) {
        self.ratio = if ratio.is_nan() {
            1.0
        } else {
            ratio.clamp(1.0, 100.0)
        };
    }

    pub fn set_attack(&mut self, milliseconds: f32) {
        let milliseconds = if milliseconds.is_finite() {
            milliseconds.max(0.1)
        } else {
            0.1
        };
        let denominator = (self.sample_rate * milliseconds as f64 * 0.001).max(1.0);
        self.attack_alpha = (-1.0 / denominator).exp() as f32;
    }

    pub fn set_release(&mut self, milliseconds: f32) {
        let milliseconds = if milliseconds.is_finite() {
            milliseconds.max(0.1)
        } else {
            0.1
        };
        let denominator = (self.sample_rate * milliseconds as f64 * 0.001).max(1.0);
        self.release_alpha = (-1.0 / denominator).exp() as f32;
    }

    pub fn set_makeup(&mut self, db: f32) {
        if db.is_finite() {
            self.makeup_db = db;
        }
    }

    pub fn set_knee(&mut self, db: f32) {
        self.knee_db = if db.is_nan() { 0.0 } else { db.max(0.0) };
    }

    fn attack_ms(&self) -> f32 {
        (-1.0 / (self.sample_rate.max(1.0e-6) * self.attack_alpha.max(1.0e-6).ln() as f64) * 1000.0)
            as f32
    }

    fn release_ms(&self) -> f32 {
        (-1.0 / (self.sample_rate.max(1.0e-6) * self.release_alpha.max(1.0e-6).ln() as f64)
            * 1000.0) as f32
    }

    fn calculate_auto_makeup(&self) -> f32 {
        -(self.threshold_db * (1.0 - 1.0 / self.ratio)) * 0.5
    }

    pub fn process(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        let frames = right
            .as_ref()
            .map_or(left.len(), |samples| left.len().min(samples.len()));
        // Keep the callback bounded: the full state audit scans both delay
        // buffers and belongs on a control/debug path, never on every block.
        if frames == 0 || frames > MAX_LOOKAHEAD * 16 {
            return;
        }
        let makeup_db = if self.auto_gain {
            self.calculate_auto_makeup()
        } else {
            self.makeup_db
        };
        let makeup = 10.0_f32.powf(makeup_db / 20.0);
        for frame in 0..frames {
            let in_l = left[frame];
            let in_r = right.as_ref().map_or(in_l, |samples| samples[frame]);
            let detector = if self.use_rms {
                (0.5 * (in_l * in_l + in_r * in_r)).sqrt()
            } else {
                in_l.abs().max(in_r.abs())
            };
            let target = if detector.is_finite() { detector } else { 0.0 };
            let alpha = if target > self.envelope {
                self.attack_alpha
            } else {
                self.release_alpha
            };
            self.envelope = alpha * self.envelope + (1.0 - alpha) * target;
            let level_db = 20.0 * self.envelope.max(1.0e-8).log10();
            let mut reduction_db = 0.0;
            if self.knee_db > 0.0
                && level_db > self.threshold_db - self.knee_db * 0.5
                && level_db < self.threshold_db + self.knee_db * 0.5
            {
                let x = level_db - self.threshold_db + self.knee_db * 0.5;
                reduction_db = (1.0 / self.ratio - 1.0) * x * x / (2.0 * self.knee_db);
            } else if level_db > self.threshold_db {
                reduction_db =
                    (self.threshold_db + (level_db - self.threshold_db) / self.ratio) - level_db;
            }
            self.current_gr = 0.995 * self.current_gr + 0.005 * 10.0_f32.powf(reduction_db / 20.0);

            self.delay_l[self.write_idx] = in_l;
            self.delay_r[self.write_idx] = in_r;
            let delay = self.lookahead_samples.min(MAX_LOOKAHEAD - 1);
            let read_idx = (self.write_idx + MAX_LOOKAHEAD - delay) % MAX_LOOKAHEAD;
            left[frame] = self.delay_l[read_idx] * self.current_gr * makeup;
            if let Some(samples) = right.as_deref_mut() {
                samples[frame] = self.delay_r[read_idx] * self.current_gr * makeup;
            }
            self.write_idx = (self.write_idx + 1) % MAX_LOOKAHEAD;
        }
    }

    pub fn get_parameter(&self, id: u32) -> f32 {
        match id {
            0 => ((self.threshold_db + 60.0) / 60.0).clamp(0.0, 1.0),
            1 => ((self.ratio - 1.0) / 19.0).clamp(0.0, 1.0),
            2 => ((self.attack_ms() - 0.1) / 99.9).clamp(0.0, 1.0),
            3 => ((self.release_ms() - 5.0) / 995.0).clamp(0.0, 1.0),
            4 => ((self.makeup_db + 12.0) / 24.0).clamp(0.0, 1.0),
            5 => (self.knee_db / 24.0).clamp(0.0, 1.0),
            6 => (self.lookahead_samples as f32 / (self.sample_rate as f32 * 0.020).max(1.0))
                .clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    pub fn set_parameter(&mut self, id: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        let value = value.clamp(0.0, 1.0);
        match id {
            0 => self.set_threshold(-60.0 + value * 60.0),
            1 => self.set_ratio(1.0 + value * 19.0),
            2 => self.set_attack(0.1 + value * 99.9),
            3 => self.set_release(5.0 + value * 995.0),
            4 => self.set_makeup(-12.0 + value * 24.0),
            5 => self.set_knee(value * 24.0),
            6 => self.set_lookahead(value * 20.0),
            _ => {}
        }
    }

    pub fn restore_settings(
        &mut self,
        threshold: f32,
        ratio: f32,
        makeup: f32,
        knee: f32,
        lookahead: u32,
    ) -> bool {
        if !threshold.is_finite()
            || !ratio.is_finite()
            || !makeup.is_finite()
            || !knee.is_finite()
            || !(-100.0..=0.0).contains(&threshold)
            || !(1.0..=100.0).contains(&ratio)
            || !(0.0..=60.0).contains(&knee)
            || lookahead as usize >= MAX_LOOKAHEAD
        {
            return false;
        }
        self.threshold_db = threshold;
        self.ratio = ratio;
        self.makeup_db = makeup;
        self.knee_db = knee;
        self.lookahead_samples = lookahead as usize;
        true
    }

    pub fn get_tail_samples(&self) -> u32 {
        let release = self.release_ms().clamp(0.1, 2000.0) as f64;
        (self.lookahead_samples as f64 + release * 0.001 * self.sample_rate * 7.0)
            .min(30.0 * self.sample_rate) as u32
    }

    pub fn audit_compressor(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.threshold_db.is_finite()
            && (-100.0..=0.0).contains(&self.threshold_db)
            && self.ratio.is_finite()
            && (1.0..=100.0).contains(&self.ratio)
            && self.makeup_db.is_finite()
            && self.knee_db.is_finite()
            && self.envelope.is_finite()
            && self.current_gr.is_finite()
            && self.delay_l.len() == MAX_LOOKAHEAD
            && self.delay_r.len() == MAX_LOOKAHEAD
            && self.lookahead_samples < MAX_LOOKAHEAD
            && self.write_idx < MAX_LOOKAHEAD
            && self
                .delay_l
                .iter()
                .chain(&self.delay_r)
                .all(|sample| sample.is_finite())
    }
}

#[no_mangle]
pub extern "C" fn hirari_dynamic_compressor_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(DynamicCompressorEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_dynamic_compressor_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<DynamicCompressorEngine>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_dynamic_compressor_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = state.cast::<DynamicCompressorEngine>().as_mut() {
        state.prepare_to_play(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_dynamic_compressor_reset(state: *mut c_void) {
    if let Some(state) = state.cast::<DynamicCompressorEngine>().as_mut() {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_dynamic_compressor_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) {
    if left.is_null() {
        return;
    }
    let Some(state) = state.cast::<DynamicCompressorEngine>().as_mut() else {
        return;
    };
    let left = std::slice::from_raw_parts_mut(left, frames);
    if right.is_null() {
        state.process(left, None);
    } else {
        state.process(left, Some(std::slice::from_raw_parts_mut(right, frames)));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_dynamic_compressor_set_parameter(
    state: *mut c_void,
    id: u32,
    value: f32,
) {
    if let Some(state) = state.cast::<DynamicCompressorEngine>().as_mut() {
        state.set_parameter(id, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_dynamic_compressor_get_parameter(
    state: *const c_void,
    id: u32,
) -> f32 {
    state
        .cast::<DynamicCompressorEngine>()
        .as_ref()
        .map_or(0.0, |state| state.get_parameter(id))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_dynamic_compressor_set_control(
    state: *mut c_void,
    control: u32,
    value: f32,
) {
    let Some(state) = state.cast::<DynamicCompressorEngine>().as_mut() else {
        return;
    };
    match control {
        0 => state.set_lookahead(value),
        1 => state.set_threshold(value),
        2 => state.set_ratio(value),
        3 => state.set_attack(value),
        4 => state.set_release(value),
        5 => state.set_makeup(value),
        6 => state.auto_gain = value != 0.0,
        7 => state.set_knee(value),
        8 => state.use_rms = value != 0.0,
        _ => {}
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_dynamic_compressor_get_control(
    state: *const c_void,
    control: u32,
) -> f32 {
    let Some(state) = state.cast::<DynamicCompressorEngine>().as_ref() else {
        return 0.0;
    };
    match control {
        0 => state.lookahead_samples as f32,
        1 => state.threshold_db,
        2 => state.ratio,
        3 => state.attack_ms(),
        4 => state.release_ms(),
        5 => state.makeup_db,
        6 => f32::from(state.auto_gain),
        7 => state.knee_db,
        8 => f32::from(state.use_rms),
        _ => 0.0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_dynamic_compressor_restore(
    state: *mut c_void,
    threshold: f32,
    ratio: f32,
    makeup: f32,
    knee: f32,
    lookahead: u32,
) -> bool {
    state
        .cast::<DynamicCompressorEngine>()
        .as_mut()
        .is_some_and(|state| state.restore_settings(threshold, ratio, makeup, knee, lookahead))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_dynamic_compressor_latency(state: *const c_void) -> u32 {
    state
        .cast::<DynamicCompressorEngine>()
        .as_ref()
        .map_or(0, |state| state.lookahead_samples as u32)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_dynamic_compressor_tail(state: *const c_void) -> u32 {
    state
        .cast::<DynamicCompressorEngine>()
        .as_ref()
        .map_or(0, DynamicCompressorEngine::get_tail_samples)
}

#[cfg(test)]
mod tests {
    use super::DynamicCompressorEngine;

    #[test]
    fn compressor_handles_mismatched_audio_and_sidechain_lengths() {
        let mut compressor = DynamicCompressorEngine::new(48_000.0);
        let mut left = vec![0.8_f32; 128];
        let mut right = vec![0.4_f32; 64];
        compressor.process(&mut left, Some(&mut right));
        assert!(left[..64].iter().chain(right.iter()).all(|v| v.is_finite()));
        assert!(compressor.audit_compressor());
    }

    #[test]
    fn compressor_audit_rejects_corrupt_state() {
        let mut compressor = DynamicCompressorEngine::new(48_000.0);
        compressor.current_gr = f32::NAN;
        assert!(!compressor.audit_compressor());
    }
}
