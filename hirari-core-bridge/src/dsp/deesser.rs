use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy)]
struct SimpleBandpass {
    z1: f32,
    z2: f32,
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Default for SimpleBandpass {
    fn default() -> Self {
        Self {
            z1: 0.0,
            z2: 0.0,
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }
}

impl SimpleBandpass {
    fn process(&mut self, input: f32) -> f32 {
        let output = self.b0 * input + self.b1 * self.z1 + self.b2 * self.z2
            - self.a1 * self.z1
            - self.a2 * self.z2;
        self.z2 = self.z1;
        self.z1 = output;
        output
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

struct DeEsserRuntime {
    sample_rate: f64,
    envelope: f32,
    current_gain: f32,
    sidechain_filter: SimpleBandpass,
}

pub struct DeEsserEngine {
    threshold: AtomicU32,
    intensity: AtomicU32,
    runtime: UnsafeCell<DeEsserRuntime>,
}

impl DeEsserEngine {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            threshold: AtomicU32::new(0.5f32.to_bits()),
            intensity: AtomicU32::new(0.8f32.to_bits()),
            runtime: UnsafeCell::new(DeEsserRuntime {
                sample_rate: if sample_rate.is_finite()
                    && (100.0..=384_000.0).contains(&sample_rate)
                {
                    sample_rate
                } else {
                    44_100.0
                },
                envelope: 0.0,
                current_gain: 1.0,
                sidechain_filter: SimpleBandpass::default(),
            }),
        }
    }

    pub fn prepare(&self, sample_rate: f64) {
        if !sample_rate.is_finite() || !(100.0..=384_000.0).contains(&sample_rate) {
            return;
        }
        // SAFETY: host prepare is serialized with the audio callback.
        let runtime = unsafe { &mut *self.runtime.get() };
        runtime.sample_rate = sample_rate;
        let omega = 2.0 * std::f64::consts::PI * 6_000.0 / runtime.sample_rate;
        let alpha = omega.sin() / 2.0;
        let a0 = 1.0 + alpha;
        runtime.sidechain_filter.b0 = (alpha / a0) as f32;
        runtime.sidechain_filter.b1 = 0.0;
        runtime.sidechain_filter.b2 = (-alpha / a0) as f32;
        runtime.sidechain_filter.a1 = (-2.0 * omega.cos() / a0) as f32;
        runtime.sidechain_filter.a2 = ((1.0 - alpha) / a0) as f32;
    }

    pub fn set_threshold(&self, threshold: f32) {
        self.threshold.store(threshold.to_bits(), Ordering::Relaxed);
    }

    pub fn set_intensity(&self, intensity: f32) {
        self.intensity.store(intensity.to_bits(), Ordering::Relaxed);
    }

    fn parameter(&self, parameter: &AtomicU32) -> f32 {
        f32::from_bits(parameter.load(Ordering::Relaxed))
    }

    pub fn reset(&self) {
        // SAFETY: host reset is serialized with the audio callback.
        let runtime = unsafe { &mut *self.runtime.get() };
        runtime.envelope = 0.0;
        runtime.current_gain = 1.0;
        runtime.sidechain_filter.reset();
    }

    pub fn tail_samples(&self) -> u32 {
        // SAFETY: sample rate only changes during serialized prepare calls.
        let sample_rate = unsafe { (*self.runtime.get()).sample_rate };
        (0.32 * sample_rate.clamp(100.0, 384_000.0)) as u32
    }

    pub fn process(&self, left: &mut [f32], right: Option<&mut [f32]>) {
        // SAFETY: the host gives one audio thread exclusive access to the DSP
        // runtime. Control values are separate atomics; reset/prepare are
        // serialized with processing by the host lifecycle.
        let runtime = unsafe { &mut *self.runtime.get() };
        let threshold = self.parameter(&self.threshold).clamp(0.001, 1.0);
        let intensity = self.parameter(&self.intensity).clamp(0.0, 2.0);
        let mut right = right;
        let frames = right
            .as_ref()
            .map_or(left.len(), |channel| left.len().min(channel.len()));

        for frame in 0..frames {
            let input_left = if left[frame].is_finite() {
                left[frame]
            } else {
                0.0
            };
            let input_right = right.as_ref().map_or(input_left, |channel| {
                if channel[frame].is_finite() {
                    channel[frame]
                } else {
                    input_left
                }
            });
            let sidechain = 0.5 * (input_left + input_right);
            let level = runtime.sidechain_filter.process(sidechain).abs();

            if level > runtime.envelope {
                runtime.envelope = 0.9 * runtime.envelope + 0.1 * level;
            } else {
                runtime.envelope = 0.999 * runtime.envelope + 0.001 * level;
            }
            if runtime.envelope.abs() < 1.0e-24 {
                runtime.envelope = 0.0;
            }

            let excess = (runtime.envelope - threshold).max(0.0);
            let target_gain = 1.0 / (1.0 + intensity * excess * 12.0);
            runtime.current_gain += (target_gain - runtime.current_gain) * 0.08;
            left[frame] = input_left * runtime.current_gain;
            if let Some(channel) = right.as_deref_mut() {
                channel[frame] = input_right * runtime.current_gain;
            }
        }
    }

    pub fn audit(&self) -> bool {
        // SAFETY: diagnostics are called off the audio thread.
        let runtime = unsafe { &*self.runtime.get() };
        runtime.sample_rate.is_finite()
            && (100.0..=384_000.0).contains(&runtime.sample_rate)
            && runtime.envelope.is_finite()
            && runtime.current_gain.is_finite()
            && runtime.sidechain_filter.z1.is_finite()
            && runtime.sidechain_filter.z2.is_finite()
    }
}

#[no_mangle]
pub extern "C" fn hirari_deesser_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(DeEsserEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_deesser_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: handle was allocated by `hirari_deesser_create`.
        unsafe { drop(Box::from_raw(state.cast::<DeEsserEngine>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_deesser_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<DeEsserEngine>().as_ref() } {
        state.prepare(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_deesser_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<DeEsserEngine>().as_ref() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_deesser_set_threshold(state: *mut c_void, value: f32) {
    if let Some(state) = unsafe { state.cast::<DeEsserEngine>().as_ref() } {
        state.set_threshold(value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_deesser_set_intensity(state: *mut c_void, value: f32) {
    if let Some(state) = unsafe { state.cast::<DeEsserEngine>().as_ref() } {
        state.set_intensity(value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_deesser_tail_samples(state: *const c_void) -> u32 {
    unsafe { state.cast::<DeEsserEngine>().as_ref() }.map_or(0, |state| state.tail_samples())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_deesser_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
    let Some(state) = (unsafe { state.cast::<DeEsserEngine>().as_ref() }) else {
        return;
    };
    if left.is_null() || frames == 0 || right == left {
        return;
    }
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames as usize) };
    if right.is_null() {
        state.process(left, None);
    } else {
        let right = unsafe { std::slice::from_raw_parts_mut(right, frames as usize) };
        state.process(left, Some(right));
    }
}

#[cfg(test)]
mod tests {
    use super::{DeEsserEngine, SimpleBandpass};
    use std::ffi::c_void;

    struct FrozenCppDeEsser {
        sample_rate: f64,
        threshold: f32,
        intensity: f32,
        envelope: f32,
        gain: f32,
        filter: SimpleBandpass,
    }

    impl FrozenCppDeEsser {
        fn new(sample_rate: f64) -> Self {
            let mut reference = Self {
                sample_rate: 44_100.0,
                threshold: 0.5,
                intensity: 0.8,
                envelope: 0.0,
                gain: 1.0,
                filter: SimpleBandpass::default(),
            };
            reference.prepare(sample_rate);
            reference
        }

        fn prepare(&mut self, sample_rate: f64) {
            if !sample_rate.is_finite() || !(100.0..=384_000.0).contains(&sample_rate) {
                return;
            }
            self.sample_rate = sample_rate;
            let omega = 2.0 * std::f64::consts::PI * 6_000.0 / self.sample_rate;
            let alpha = omega.sin() / 2.0;
            let a0 = 1.0 + alpha;
            self.filter.b0 = (alpha / a0) as f32;
            self.filter.b1 = 0.0;
            self.filter.b2 = (-alpha / a0) as f32;
            self.filter.a1 = (-2.0 * omega.cos() / a0) as f32;
            self.filter.a2 = ((1.0 - alpha) / a0) as f32;
        }

        fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
            for frame in 0..left.len().min(right.len()) {
                let in_left = if left[frame].is_finite() {
                    left[frame]
                } else {
                    0.0
                };
                let in_right = if right[frame].is_finite() {
                    right[frame]
                } else {
                    in_left
                };
                let level = self.filter.process(0.5 * (in_left + in_right)).abs();
                if level > self.envelope {
                    self.envelope = 0.9 * self.envelope + 0.1 * level;
                } else {
                    self.envelope = 0.999 * self.envelope + 0.001 * level;
                }
                if self.envelope.abs() < 1.0e-24 {
                    self.envelope = 0.0;
                }
                let excess = (self.envelope - self.threshold.clamp(0.001, 1.0)).max(0.0);
                let target = 1.0 / (1.0 + self.intensity.clamp(0.0, 2.0) * excess * 12.0);
                self.gain += (target - self.gain) * 0.08;
                left[frame] = in_left * self.gain;
                right[frame] = in_right * self.gain;
            }
        }
    }

    #[test]
    fn stereo_blocks_match_frozen_cpp_filter_envelope_and_gain() {
        let rust = DeEsserEngine::new(44_100.0);
        rust.prepare(48_000.0);
        rust.set_threshold(0.12);
        rust.set_intensity(1.4);
        let mut cpp = FrozenCppDeEsser::new(44_100.0);
        cpp.prepare(48_000.0);
        cpp.threshold = 0.12;
        cpp.intensity = 1.4;

        let source_left = (0..257)
            .map(|index| (index as f32 * 0.39).sin() * 0.72)
            .collect::<Vec<_>>();
        let source_right = (0..257)
            .map(|index| (index as f32 * 0.41).cos() * 0.68)
            .collect::<Vec<_>>();
        let mut rust_left = source_left.clone();
        let mut rust_right = source_right.clone();
        let mut cpp_left = source_left;
        let mut cpp_right = source_right;

        rust.process(&mut rust_left[..73], Some(&mut rust_right[..73]));
        cpp.process(&mut cpp_left[..73], &mut cpp_right[..73]);
        rust.process(&mut rust_left[73..], Some(&mut rust_right[73..]));
        cpp.process(&mut cpp_left[73..], &mut cpp_right[73..]);
        for (actual, expected) in rust_left.iter().zip(&cpp_left) {
            assert!((actual - expected).abs() <= 1.0e-7);
        }
        for (actual, expected) in rust_right.iter().zip(&cpp_right) {
            assert!((actual - expected).abs() <= 1.0e-7);
        }
        assert!(rust.audit());
    }

    #[test]
    fn mono_processing_and_tail_reporting_match_native_contract() {
        let engine = DeEsserEngine::new(48_000.0);
        engine.prepare(48_000.0);
        assert_eq!(engine.tail_samples(), (48_000.0f64 * 0.32) as u32);
        let mut mono = [0.2f32, f32::NAN, -0.1, 0.8];
        engine.process(&mut mono, None);
        assert!(mono.iter().all(|sample| sample.is_finite()));
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn production_kernel_matches_frozen_cpp_reference_pcm() {
        unsafe extern "C" {
            fn hirari_deesser_reference_create() -> *mut c_void;
            fn hirari_deesser_reference_destroy(state: *mut c_void);
            fn hirari_deesser_reference_prepare(state: *mut c_void, sample_rate: f64);
            fn hirari_deesser_reference_set_parameters(
                state: *mut c_void,
                threshold: f32,
                intensity: f32,
            );
            fn hirari_deesser_reference_tail_samples(state: *const c_void) -> u32;
            fn hirari_deesser_reference_process(
                state: *mut c_void,
                left: *mut f32,
                right: *mut f32,
                frames: u32,
            );
        }

        let rust = DeEsserEngine::new(44_100.0);
        rust.prepare(48_000.0);
        rust.set_threshold(0.12);
        rust.set_intensity(1.4);
        let cpp = unsafe { hirari_deesser_reference_create() };
        assert!(!cpp.is_null());
        unsafe {
            hirari_deesser_reference_prepare(cpp, 48_000.0);
            hirari_deesser_reference_set_parameters(cpp, 0.12, 1.4);
        }
        assert_eq!(rust.tail_samples(), unsafe {
            hirari_deesser_reference_tail_samples(cpp)
        });

        let source_left = (0..513)
            .map(|index| (index as f32 * 0.137).sin() * 0.91)
            .collect::<Vec<_>>();
        let source_right = (0..513)
            .map(|index| (index as f32 * 0.211).cos() * 0.83)
            .collect::<Vec<_>>();
        let mut rust_left = source_left.clone();
        let mut rust_right = source_right.clone();
        let mut cpp_left = source_left;
        let mut cpp_right = source_right;

        for range in [0..31, 31..257, 257..513] {
            let frames = (range.end - range.start) as u32;
            rust.process(
                &mut rust_left[range.clone()],
                Some(&mut rust_right[range.clone()]),
            );
            unsafe {
                hirari_deesser_reference_process(
                    cpp,
                    cpp_left[range.clone()].as_mut_ptr(),
                    cpp_right[range.clone()].as_mut_ptr(),
                    frames,
                )
            };
        }
        for (actual, expected) in rust_left.iter().zip(&cpp_left) {
            assert!((actual - expected).abs() <= 2.0e-7);
        }
        for (actual, expected) in rust_right.iter().zip(&cpp_right) {
            assert!((actual - expected).abs() <= 2.0e-7);
        }
        unsafe { hirari_deesser_reference_destroy(cpp) };
    }
}
