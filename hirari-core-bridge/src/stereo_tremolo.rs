use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

struct TremoloRuntime {
    sample_rate: f64,
    lfo_phase: f64,
}

pub struct StereoTremoloEngine {
    runtime: UnsafeCell<TremoloRuntime>,
    depth: AtomicU32,
    note_value: AtomicU32,
    stereo_width: AtomicU32,
}

impl StereoTremoloEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            runtime: UnsafeCell::new(TremoloRuntime {
                sample_rate: if sr.is_finite() && (100.0..=384_000.0).contains(&sr) {
                    sr
                } else {
                    44_100.0
                },
                lfo_phase: 0.0,
            }),
            depth: AtomicU32::new(0.0f32.to_bits()),
            note_value: AtomicU32::new(0.25f32.to_bits()),
            stereo_width: AtomicU32::new(0.5f32.to_bits()),
        }
    }

    fn load(parameter: &AtomicU32) -> f32 {
        f32::from_bits(parameter.load(Ordering::Relaxed))
    }

    pub fn reset(&self) {
        // SAFETY: Reset/prepare must be serialized with audio processing by the host.
        unsafe {
            (*self.runtime.get()).lfo_phase = 0.0;
        }
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
        runtime.lfo_phase = 0.0;
    }

    pub fn depth(&self) -> f32 {
        Self::load(&self.depth)
    }
    pub fn note_value(&self) -> f32 {
        Self::load(&self.note_value)
    }
    pub fn stereo_width(&self) -> f32 {
        Self::load(&self.stereo_width)
    }

    pub fn set_depth(&self, d: f32) {
        let value = if d.is_finite() {
            d.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.depth.store(value.to_bits(), Ordering::Relaxed);
    }

    pub fn set_note_value(&self, v: f32) {
        let value = if v.is_finite() {
            v.clamp(0.0625, 2.0)
        } else {
            0.25
        };
        self.note_value.store(value.to_bits(), Ordering::Relaxed);
    }

    pub fn set_stereo_width(&self, w: f32) {
        let value = if w.is_finite() {
            w.clamp(0.0, 1.0)
        } else {
            0.5
        };
        self.stereo_width.store(value.to_bits(), Ordering::Relaxed);
    }

    /// INDUSTRIAL: Rhythmic volume and pan modulation.
    pub fn process(&self, l: &mut [f32], r: &mut [f32], bpm: f64, current_sample_rate: f64) {
        let len = l.len().min(r.len());
        if len == 0 {
            return;
        }
        let sample_rate = if current_sample_rate.is_finite() && current_sample_rate > 1_000.0 {
            current_sample_rate
        } else {
            44_100.0
        };
        let bpm = if bpm.is_finite() && bpm > 1.0 {
            bpm
        } else {
            120.0
        };
        let note = Self::load(&self.note_value);
        let depth = Self::load(&self.depth);
        let width = Self::load(&self.stereo_width);
        let note_value = if note.is_finite() {
            (note as f64).clamp(0.0625, 2.0)
        } else {
            0.25
        };
        let hz = bpm / (60.0 * note_value);
        let increment = hz / sample_rate;
        if !increment.is_finite() {
            return;
        }

        // SAFETY: The host calls process from one audio thread; prepare/reset are
        // serialized with it, while parameter writers only touch atomic controls.
        let runtime = unsafe { &mut *self.runtime.get() };
        for s in 0..len {
            let phase = runtime.lfo_phase;
            let lfo = (0.5 + 0.5 * (2.0 * std::f64::consts::PI * phase).sin()) as f32;
            let amplitude = 1.0 - depth * (1.0 - lfo);
            let pan = width * (2.0 * std::f64::consts::PI * phase).sin() as f32;
            l[s] *= amplitude * (1.0 - 0.25 * pan);
            r[s] *= amplitude * (1.0 + 0.25 * pan);
            runtime.lfo_phase += increment;
            if runtime.lfo_phase >= 1.0 {
                runtime.lfo_phase -= runtime.lfo_phase.floor();
            }
        }
    }

    pub fn process_mono(&self, samples: &mut [f32], bpm: f64, current_sample_rate: f64) {
        let sample_rate = if current_sample_rate.is_finite() && current_sample_rate > 1_000.0 {
            current_sample_rate
        } else {
            44_100.0
        };
        let bpm = if bpm.is_finite() && bpm > 1.0 {
            bpm
        } else {
            120.0
        };
        let note = Self::load(&self.note_value);
        let depth = Self::load(&self.depth);
        let width = Self::load(&self.stereo_width);
        let note_value = if note.is_finite() {
            (note as f64).clamp(0.0625, 2.0)
        } else {
            0.25
        };
        let hz = bpm / (60.0 * note_value);
        let increment = hz / sample_rate;
        if !increment.is_finite() {
            return;
        }
        // SAFETY: The host calls process from one audio thread; prepare/reset are
        // serialized with it, while parameter writers only touch atomic controls.
        let runtime = unsafe { &mut *self.runtime.get() };
        for sample in samples {
            let phase = runtime.lfo_phase;
            let lfo = (0.5 + 0.5 * (2.0 * std::f64::consts::PI * phase).sin()) as f32;
            let amplitude = 1.0 - depth * (1.0 - lfo);
            let pan = width * (2.0 * std::f64::consts::PI * phase).sin() as f32;
            *sample *= amplitude * (1.0 - 0.25 * pan);
            runtime.lfo_phase += increment;
            if runtime.lfo_phase >= 1.0 {
                runtime.lfo_phase -= runtime.lfo_phase.floor();
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Stereo Tremolo state.
    pub fn audit_stereo_tremolo(&self) -> bool {
        // SAFETY: Diagnostics are read on a non-audio control path.
        let runtime = unsafe { &*self.runtime.get() };
        let depth = self.depth();
        let note = self.note_value();
        let width = self.stereo_width();
        runtime.sample_rate.is_finite()
            && runtime.sample_rate > 100.0
            && runtime.sample_rate <= 384_000.0
            && runtime.lfo_phase.is_finite()
            && depth.is_finite()
            && (0.0..=1.0).contains(&depth)
            && note.is_finite()
            && (0.0625..=2.0).contains(&note)
            && width.is_finite()
            && (0.0..=1.0).contains(&width)
    }
}

#[no_mangle]
pub extern "C" fn hirari_stereo_tremolo_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(StereoTremoloEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_tremolo_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: `state` was allocated by `hirari_stereo_tremolo_create`.
        unsafe { drop(Box::from_raw(state.cast::<StereoTremoloEngine>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_tremolo_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<StereoTremoloEngine>().as_ref() } {
        state.prepare(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_tremolo_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<StereoTremoloEngine>().as_ref() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_tremolo_set_parameter(
    state: *mut c_void,
    id: u32,
    value: f32,
) {
    if let Some(state) = unsafe { state.cast::<StereoTremoloEngine>().as_ref() } {
        if !value.is_finite() {
            return;
        }
        match id {
            0 => state.set_depth(value),
            1 => state.set_note_value(0.0625 + value.clamp(0.0, 1.0) * 1.9375),
            2 => state.set_stereo_width(value),
            _ => {}
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_tremolo_get_parameter(state: *const c_void, id: u32) -> f32 {
    let Some(state) = (unsafe { state.cast::<StereoTremoloEngine>().as_ref() }) else {
        return 0.0;
    };
    match id {
        0 => state.depth(),
        1 => ((state.note_value() - 0.0625) / 1.9375).clamp(0.0, 1.0),
        2 => state.stereo_width(),
        _ => 0.0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_tremolo_get_note_value(state: *const c_void) -> f32 {
    unsafe { state.cast::<StereoTremoloEngine>().as_ref() }
        .map_or(0.25, StereoTremoloEngine::note_value)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_tremolo_set_depth(state: *mut c_void, value: f32) {
    if let Some(state) = unsafe { state.cast::<StereoTremoloEngine>().as_ref() } {
        state.set_depth(value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_tremolo_set_note_value(state: *mut c_void, value: f32) {
    if let Some(state) = unsafe { state.cast::<StereoTremoloEngine>().as_ref() } {
        state.set_note_value(value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_tremolo_set_width(state: *mut c_void, value: f32) {
    if let Some(state) = unsafe { state.cast::<StereoTremoloEngine>().as_ref() } {
        state.set_stereo_width(value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_stereo_tremolo_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    bpm: f64,
) {
    let Some(state) = (unsafe { state.cast::<StereoTremoloEngine>().as_ref() }) else {
        return;
    };
    if left.is_null() || frames == 0 {
        return;
    }
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames as usize) };
    // SAFETY: The audio callback is the sole owner of the runtime state.
    let sample_rate = unsafe { (*state.runtime.get()).sample_rate };
    if right.is_null() {
        state.process_mono(left, bpm, sample_rate);
        return;
    }
    let right = unsafe { std::slice::from_raw_parts_mut(right, frames as usize) };
    state.process(left, right, bpm, sample_rate);
}

#[cfg(test)]
mod tests {
    use super::{
        hirari_stereo_tremolo_create, hirari_stereo_tremolo_destroy, hirari_stereo_tremolo_process,
        hirari_stereo_tremolo_set_depth, hirari_stereo_tremolo_set_note_value,
        hirari_stereo_tremolo_set_width, StereoTremoloEngine,
    };

    #[test]
    fn setters_reject_non_finite_and_bound_parameters() {
        let engine = StereoTremoloEngine::new(f64::NAN);
        assert!(engine.audit_stereo_tremolo());

        engine.set_depth(f32::NAN);
        engine.set_note_value(f32::INFINITY);
        engine.set_stereo_width(f32::NEG_INFINITY);

        assert_eq!(engine.depth(), 0.0);
        assert_eq!(engine.note_value(), 0.25);
        assert_eq!(engine.stereo_width(), 0.5);
        assert!(engine.audit_stereo_tremolo());
    }

    #[test]
    fn process_matches_native_tremolo_formula() {
        let engine = StereoTremoloEngine::new(44_100.0);
        engine.set_depth(0.8);
        engine.set_note_value(0.5);
        engine.set_stereo_width(0.7);
        unsafe {
            (*engine.runtime.get()).lfo_phase = 0.15;
        }
        let mut left = [0.25, -0.5, 0.75, -1.0];
        let mut right = [-0.25, 0.5, -0.75, 1.0];
        let mut expected_left = left;
        let mut expected_right = right;
        let increment = (123.0 / (60.0 * 0.5)) / 48_000.0;
        let mut phase: f64 = 0.15;
        for index in 0..left.len() {
            let lfo = (0.5 + 0.5 * (2.0 * std::f64::consts::PI * phase).sin()) as f32;
            let amplitude = 1.0 - 0.8 * (1.0 - lfo);
            let pan = 0.7 * (2.0 * std::f64::consts::PI * phase).sin() as f32;
            expected_left[index] *= amplitude * (1.0 - 0.25 * pan);
            expected_right[index] *= amplitude * (1.0 + 0.25 * pan);
            phase += increment;
            if phase >= 1.0 {
                phase -= phase.floor();
            }
        }

        engine.process(&mut left, &mut right, 123.0, 48_000.0);

        assert_eq!(left, expected_left);
        assert_eq!(right, expected_right);
        assert!((unsafe { (*engine.runtime.get()).lfo_phase } - phase).abs() < 1e-12);
    }

    #[test]
    fn invalid_timing_uses_native_fallback_values() {
        let engine = StereoTremoloEngine::new(44_100.0);
        let mut left = [0.25, -0.5];
        let mut right = [0.5, -0.25];
        engine.set_depth(0.8);
        let mut expected_left = left;
        let mut expected_right = right;
        let mut phase = 0.0f64;
        let increment = (120.0 / (60.0 * 0.25)) / 44_100.0;
        for index in 0..left.len() {
            let lfo = (0.5 + 0.5 * (2.0 * std::f64::consts::PI * phase).sin()) as f32;
            let amplitude = 1.0 - 0.8 * (1.0 - lfo);
            let pan = 0.5 * (2.0 * std::f64::consts::PI * phase).sin() as f32;
            expected_left[index] *= amplitude * (1.0 - 0.25 * pan);
            expected_right[index] *= amplitude * (1.0 + 0.25 * pan);
            phase += increment;
        }

        engine.process(&mut left, &mut right, f64::NAN, 44_100.0);

        assert_eq!(left, expected_left);
        assert_eq!(right, expected_right);
    }

    #[test]
    fn ffi_process_uses_the_same_rust_owned_state_and_parameters() {
        let state = hirari_stereo_tremolo_create(48_000.0);
        assert!(!state.is_null());
        unsafe {
            hirari_stereo_tremolo_set_depth(state, 0.6);
            hirari_stereo_tremolo_set_note_value(state, 0.375);
            hirari_stereo_tremolo_set_width(state, 0.4);
        }
        let mut left = [0.2, 0.4, 0.6];
        let mut right = [-0.2, -0.4, -0.6];
        unsafe {
            hirari_stereo_tremolo_process(
                state,
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                left.len() as u32,
                97.0,
            );
        }
        let expected = StereoTremoloEngine::new(48_000.0);
        expected.set_depth(0.6);
        expected.set_note_value(0.375);
        expected.set_stereo_width(0.4);
        let mut expected_left = [0.2, 0.4, 0.6];
        let mut expected_right = [-0.2, -0.4, -0.6];
        expected.process(&mut expected_left, &mut expected_right, 97.0, 48_000.0);
        assert_eq!(left, expected_left);
        assert_eq!(right, expected_right);
        unsafe { hirari_stereo_tremolo_destroy(state) };
    }
}
