pub struct PitchShifter {
    pub delay_buf: [f32; 8192],
    pub write_idx: usize,
    pub phase1: f32,
    pub phase2: f32,
}

impl Default for PitchShifter {
    fn default() -> Self {
        Self::new()
    }
}

impl PitchShifter {
    pub fn new() -> Self {
        Self {
            delay_buf: [0.0; 8192],
            write_idx: 0,
            phase1: 0.0,
            phase2: 4096.0, // Offset by 180 degrees
        }
    }

    pub fn reset(&mut self) {
        self.delay_buf.fill(0.0);
        self.write_idx = 0;
        self.phase1 = 0.0;
        self.phase2 = 4096.0;
    }

    pub fn process(&mut self, buffer: &mut [f32], pitch_ratio: f32) {
        if !pitch_ratio.is_finite() || !(0.25..=4.0).contains(&pitch_ratio) {
            return;
        }
        if (pitch_ratio - 1.0).abs() < 0.001 {
            return;
        }

        let len = buffer.len();
        let mask = 8191;

        for s in 0..len {
            let in_val = buffer[s];
            self.delay_buf[self.write_idx] = in_val;

            // Dual delay-tap crossfading to prevent clicks
            let mut tap1 = self.write_idx as f32 - self.phase1;
            let mut tap2 = self.write_idx as f32 - self.phase2;

            // Circular wrap
            while tap1 < 0.0 {
                tap1 += 8192.0;
            }
            while tap2 < 0.0 {
                tap2 += 8192.0;
            }

            // Simple Linear Interpolation
            let i0_1 = tap1 as usize & mask;
            let i0_2 = tap2 as usize & mask;
            let out1 = self.delay_buf[i0_1];
            let out2 = self.delay_buf[i0_2];

            // Crossfade window calculation
            let window = (self.phase1 - 4096.0).abs() / 4096.0;
            let final_out = (out1 * window) + (out2 * (1.0 - window));

            buffer[s] = final_out;

            // Advance phases
            self.phase1 += 1.0 - pitch_ratio;
            self.phase2 += 1.0 - pitch_ratio;

            // Wrap phases
            if self.phase1 >= 8192.0 {
                self.phase1 -= 8192.0;
            }
            if self.phase1 < 0.0 {
                self.phase1 += 8192.0;
            }
            if self.phase2 >= 8192.0 {
                self.phase2 -= 8192.0;
            }
            if self.phase2 < 0.0 {
                self.phase2 += 8192.0;
            }

            self.write_idx = (self.write_idx + 1) & mask;
        }
    }
}

pub enum ZdfFilterType {
    LowPass,
    HighPass,
    BandPass,
    Notch,
}

pub struct ZdfFilter {
    pub s1: f32,
    pub s2: f32,
    pub sample_rate: f64,
    pub g: f32,
    pub k: f32,
    pub a1: f32,
    pub a2: f32,
    pub a3: f32,
    pub filter_type: ZdfFilterType,
}

impl Default for ZdfFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl ZdfFilter {
    pub fn new() -> Self {
        Self {
            s1: 0.0,
            s2: 0.0,
            sample_rate: 44100.0,
            g: 0.0,
            k: 0.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            filter_type: ZdfFilterType::LowPass,
        }
    }

    pub fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }

    pub fn update(&mut self, cutoff: f32, resonance: f32, filter_type: ZdfFilterType) {
        if !self.sample_rate.is_finite() || self.sample_rate <= 100.0 {
            return;
        }
        let cutoff = if cutoff.is_finite() {
            cutoff.clamp(5.0, self.sample_rate as f32 * 0.49)
        } else {
            1_000.0
        };
        let resonance = if resonance.is_finite() {
            resonance.clamp(0.0, 0.99)
        } else {
            0.0
        };
        let g = (std::f32::consts::PI * cutoff / self.sample_rate as f32).tan();
        let r = 1.0 - resonance;
        self.g = g;
        self.k = 2.0 * r;
        self.filter_type = filter_type;

        self.a1 = 1.0 / (1.0 + self.g * (self.g + self.k));
        self.a2 = self.g * self.a1;
        self.a3 = self.g * self.a2;
    }

    pub fn process(&mut self, in_val: f32) -> f32 {
        if !in_val.is_finite()
            || !self.s1.is_finite()
            || !self.s2.is_finite()
            || !self.a1.is_finite()
            || !self.a2.is_finite()
            || !self.a3.is_finite()
            || !self.k.is_finite()
        {
            self.reset();
            return 0.0;
        }
        let v3 = in_val - self.s2;
        let v1 = self.a1 * self.s1 + self.a2 * v3;
        let v2 = self.s2 + self.a2 * self.s1 + self.a3 * v3;

        self.s1 = 2.0 * v1 - self.s1;
        self.s2 = 2.0 * v2 - self.s2;

        let out = match self.filter_type {
            ZdfFilterType::LowPass => v2,
            ZdfFilterType::HighPass => in_val - self.k * v1 - v2,
            ZdfFilterType::BandPass => v1,
            ZdfFilterType::Notch => in_val - self.k * v1,
        };
        if out.is_finite() {
            out
        } else {
            self.reset();
            0.0
        }
    }
}

pub struct VirtuosoVocalEngine {
    pub sample_rate: f64,
    pub shifter_l: PitchShifter,
    pub shifter_r: PitchShifter,
    pub formant_filter_l: ZdfFilter,
    pub formant_filter_r: ZdfFilter,
    pub pitch_shift_semi: f32,
    pub formant_shift: f32,
}

impl VirtuosoVocalEngine {
    pub fn new(sr: f64) -> Self {
        let sr = if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) {
            sr
        } else {
            44_100.0
        };
        let mut formant_filter_l = ZdfFilter::new();
        let mut formant_filter_r = ZdfFilter::new();
        formant_filter_l.sample_rate = sr;
        formant_filter_r.sample_rate = sr;

        Self {
            sample_rate: sr,
            shifter_l: PitchShifter::new(),
            shifter_r: PitchShifter::new(),
            formant_filter_l,
            formant_filter_r,
            pitch_shift_semi: 0.0,
            formant_shift: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.shifter_l.reset();
        self.shifter_r.reset();
        self.formant_filter_l.reset();
        self.formant_filter_r.reset();
    }

    pub fn prepare(&mut self, sample_rate: f64) {
        self.sample_rate =
            if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
                sample_rate
            } else {
                44_100.0
            };
        self.formant_filter_l.sample_rate = self.sample_rate;
        self.formant_filter_r.sample_rate = self.sample_rate;
        self.reset();
    }

    fn update_formant_filters(&mut self) {
        let shift = if self.formant_shift.is_finite() {
            self.formant_shift.clamp(-12.0, 12.0)
        } else {
            0.0
        };
        let cutoff = (3_000.0 * 2.0f32.powf(shift / 12.0)).clamp(200.0, 18_000.0);
        self.formant_filter_l
            .update(cutoff, 0.707, ZdfFilterType::BandPass);
        self.formant_filter_r
            .update(cutoff, 0.707, ZdfFilterType::BandPass);
    }

    /// INDUSTRIAL: High-end Pitch & Formant Shifter (Vocal Transformer).
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if !self.sample_rate.is_finite() || !(8_000.0..=384_000.0).contains(&self.sample_rate) {
            return;
        }
        let shift = if self.pitch_shift_semi.is_finite() {
            self.pitch_shift_semi.clamp(-24.0, 24.0)
        } else {
            0.0
        };
        let pitch_ratio = 2.0f32.powf(shift / 12.0);

        // --- 1. Pitch Shifting ---
        self.shifter_l.process(l, pitch_ratio);
        self.shifter_r.process(r, pitch_ratio);

        // --- 2. Formant Shifting (Band-Pass Peak Shifting) ---
        self.update_formant_filters();

        let len = l.len().min(r.len());
        for s in 0..len {
            let input_l = if l[s].is_finite() { l[s] } else { 0.0 };
            let input_r = if r[s].is_finite() { r[s] } else { 0.0 };
            let output_l = self.formant_filter_l.process(input_l);
            let output_r = self.formant_filter_r.process(input_r);
            l[s] = if output_l.abs() < 1.0e-24 {
                0.0
            } else {
                output_l
            };
            r[s] = if output_r.abs() < 1.0e-24 {
                0.0
            } else {
                output_r
            };
        }
    }

    pub fn process_mono(&mut self, left: &mut [f32]) {
        if !self.sample_rate.is_finite() || !(8_000.0..=384_000.0).contains(&self.sample_rate) {
            return;
        }
        let shift = if self.pitch_shift_semi.is_finite() {
            self.pitch_shift_semi.clamp(-24.0, 24.0)
        } else {
            0.0
        };
        self.shifter_l.process(left, 2.0f32.powf(shift / 12.0));
        self.update_formant_filters();
        for sample in left {
            let input = if sample.is_finite() { *sample } else { 0.0 };
            let output = self.formant_filter_l.process(input);
            *sample = if output.abs() < 1.0e-24 { 0.0 } else { output };
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Virtuoso Vocal state.
    pub fn audit_virtuoso_vocal(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.pitch_shift_semi.is_finite()
            && (-48.0..=48.0).contains(&self.pitch_shift_semi)
            && self.formant_shift.is_finite()
            && (-48.0..=48.0).contains(&self.formant_shift)
            && self.shifter_l.write_idx < self.shifter_l.delay_buf.len()
            && self.shifter_r.write_idx < self.shifter_r.delay_buf.len()
            && self.shifter_l.phase1.is_finite()
            && self.shifter_l.phase2.is_finite()
            && self.shifter_r.phase1.is_finite()
            && self.shifter_r.phase2.is_finite()
            && self.formant_filter_l.sample_rate == self.sample_rate
            && self.formant_filter_r.sample_rate == self.sample_rate
            && [&self.formant_filter_l, &self.formant_filter_r]
                .iter()
                .all(|f| {
                    f.s1.is_finite()
                        && f.s2.is_finite()
                        && f.g.is_finite()
                        && f.k.is_finite()
                        && f.a1.is_finite()
                        && f.a2.is_finite()
                        && f.a3.is_finite()
                })
    }
}

#[no_mangle]
pub extern "C" fn hirari_virtuoso_vocal_create(sample_rate: f64) -> *mut c_void {
    let engine = VirtuosoVocalEngine::new(sample_rate);
    Box::into_raw(Box::new(VirtuosoVocalFfiState {
        engine: UnsafeCell::new(engine),
        pitch_shift_bits: AtomicU32::new(0.0f32.to_bits()),
        formant_shift_bits: AtomicU32::new(0.0f32.to_bits()),
    }))
    .cast()
}

struct VirtuosoVocalFfiState {
    engine: UnsafeCell<VirtuosoVocalEngine>,
    pitch_shift_bits: AtomicU32,
    formant_shift_bits: AtomicU32,
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_vocal_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<VirtuosoVocalFfiState>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_vocal_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<VirtuosoVocalFfiState>().as_ref() } {
        unsafe { &mut *state.engine.get() }.prepare(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_vocal_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<VirtuosoVocalFfiState>().as_ref() } {
        unsafe { &mut *state.engine.get() }.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_vocal_set_pitch(state: *mut c_void, semitones: f32) {
    if let Some(state) = unsafe { state.cast::<VirtuosoVocalFfiState>().as_ref() } {
        state
            .pitch_shift_bits
            .store(semitones.to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_vocal_set_formant(state: *mut c_void, semitones: f32) {
    if let Some(state) = unsafe { state.cast::<VirtuosoVocalFfiState>().as_ref() } {
        state
            .formant_shift_bits
            .store(semitones.to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_vocal_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) {
    let Some(state) = (unsafe { state.cast::<VirtuosoVocalFfiState>().as_ref() }) else {
        return;
    };
    if frames == 0 || left.is_null() {
        return;
    }
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames) };
    // UI setters publish through atomics; the audio callback owns DSP state.
    let engine = unsafe { &mut *state.engine.get() };
    engine.pitch_shift_semi = f32::from_bits(state.pitch_shift_bits.load(Ordering::Relaxed));
    engine.formant_shift = f32::from_bits(state.formant_shift_bits.load(Ordering::Relaxed));
    if right.is_null() {
        engine.process_mono(left);
    } else {
        let right = unsafe { std::slice::from_raw_parts_mut(right, frames) };
        engine.process(left, right);
    }
}

#[cfg(test)]
mod tests {
    use super::VirtuosoVocalEngine;

    #[test]
    fn process_handles_mismatched_buffers_and_audits_state() {
        let mut e = VirtuosoVocalEngine::new(48_000.0);
        e.pitch_shift_semi = 7.0;
        let mut l = vec![0.2; 32];
        let mut r = vec![0.2; 17];
        e.process(&mut l, &mut r);
        assert!(l[..17].iter().all(|v| v.is_finite()));
        assert!(e.audit_virtuoso_vocal());
    }

    #[test]
    fn invalid_state_is_rejected() {
        let mut e = VirtuosoVocalEngine::new(48_000.0);
        e.pitch_shift_semi = f32::NAN;
        assert!(!e.audit_virtuoso_vocal());
    }
}
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
