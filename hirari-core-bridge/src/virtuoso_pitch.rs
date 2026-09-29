use crate::virtuoso_vocal::PitchShifter;
use std::ffi::c_void;

pub struct VirtuosoPitchEngine {
    pub sample_rate: f64,
    pub shifter_l: PitchShifter,
    pub shifter_r: PitchShifter,
    pub last_cross: u64,
    pub last_in: f32,
    pub detected_pitch: f32,
    pub target_pitch: f32,
}

impl VirtuosoPitchEngine {
    pub fn new(sr: f64) -> Self {
        let sample_rate = if sr.is_finite() && sr > 1_000.0 {
            sr
        } else {
            44_100.0
        };
        Self {
            sample_rate,
            shifter_l: PitchShifter::new(),
            shifter_r: PitchShifter::new(),
            last_cross: 0,
            last_in: 0.0,
            detected_pitch: 440.0,
            target_pitch: 440.0,
        }
    }

    pub fn prepare(&mut self, sample_rate: f64) {
        if sample_rate.is_finite() && sample_rate > 1_000.0 {
            self.sample_rate = sample_rate;
        }
        self.reset();
    }

    pub fn reset(&mut self) {
        self.last_cross = 0;
        self.last_in = 0.0;
        self.detected_pitch = 440.0;
        self.target_pitch = 440.0;
        self.shifter_l.reset();
        self.shifter_r.reset();
    }

    fn snap_to_scale(&self, freq: f32) -> f32 {
        let semitones = 69.0 + 12.0 * (freq / 440.0).log2();
        let nearest = semitones.round();
        440.0 * 2.0f32.powf((nearest - 69.0) / 12.0)
    }

    // Match the C++ processor's block-local rising-zero-crossing detector.
    fn detect_pitch(&mut self, left: &mut [f32]) {
        for (index, sample) in left.iter_mut().enumerate() {
            let input = if sample.is_finite() { *sample } else { 0.0 };
            let sample_index = index as u64;
            if self.last_in <= 0.0 && input > 0.0 && sample_index > self.last_cross {
                let period = sample_index - self.last_cross;
                if period > 4 && period < (self.sample_rate / 20.0) as u64 {
                    self.detected_pitch = (self.sample_rate / period as f64) as f32;
                    self.target_pitch = self.snap_to_scale(self.detected_pitch);
                    self.last_cross = sample_index;
                }
            }
            self.last_in = input;
        }
    }

    fn shift_ratio(&self) -> f32 {
        let ratio = if self.detected_pitch.is_finite() && self.detected_pitch > 20.0 {
            self.target_pitch / self.detected_pitch
        } else {
            1.0
        };
        ratio.clamp(0.5, 2.0)
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.detect_pitch(left);
        let ratio = self.shift_ratio();
        self.shifter_l.process(left, ratio);
        self.shifter_r.process(right, ratio);
    }

    pub fn process_mono(&mut self, left: &mut [f32]) {
        self.detect_pitch(left);
        let ratio = self.shift_ratio();
        self.shifter_l.process(left, ratio);
    }

    pub fn audit_virtuoso_pitch(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 1_000.0
            && self.last_in.is_finite()
            && self.detected_pitch.is_finite()
            && self.target_pitch.is_finite()
            && self.shifter_l.write_idx < self.shifter_l.delay_buf.len()
            && self.shifter_r.write_idx < self.shifter_r.delay_buf.len()
    }
}

#[no_mangle]
pub extern "C" fn hirari_virtuoso_pitch_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(VirtuosoPitchEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_pitch_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<VirtuosoPitchEngine>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_pitch_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<VirtuosoPitchEngine>().as_mut() } {
        state.prepare(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_pitch_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<VirtuosoPitchEngine>().as_mut() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_pitch_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) {
    let Some(state) = (unsafe { state.cast::<VirtuosoPitchEngine>().as_mut() }) else {
        return;
    };
    if frames == 0 || left.is_null() {
        return;
    }
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames) };
    if right.is_null() {
        state.process_mono(left);
    } else {
        let right = unsafe { std::slice::from_raw_parts_mut(right, frames) };
        state.process(left, right);
    }
}

#[cfg(test)]
mod tests {
    use super::VirtuosoPitchEngine;

    #[test]
    fn mismatched_buffers_are_safe() {
        let mut engine = VirtuosoPitchEngine::new(48_000.0);
        let mut left = vec![0.1; 32];
        let mut right = vec![0.1; 7];
        engine.process(&mut left, &mut right);
        assert!(engine.audit_virtuoso_pitch());
    }
}
