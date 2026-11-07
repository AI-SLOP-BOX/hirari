//! Realtime look-ahead limiter used on the engine's master output.
//! All delay memory is owned by this Rust state and allocated at construction.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

const LOOKAHEAD_CAPACITY: usize = 2048;
const LOOKAHEAD_MASK: usize = LOOKAHEAD_CAPACITY - 1;
pub struct MasterLimiter {
    sample_rate: f64,
    threshold_gain: AtomicU32,
    ceiling: AtomicU32,
    release_ms: AtomicU32,
    lookahead_ms: AtomicU32,
    delay_left: [f32; LOOKAHEAD_CAPACITY],
    delay_right: [f32; LOOKAHEAD_CAPACITY],
    write_index: usize,
    current_gain: f32,
}

impl MasterLimiter {
    fn new(sample_rate: f64) -> Self {
        let mut limiter = Self {
            sample_rate: 44_100.0,
            threshold_gain: AtomicU32::new(1.0f32.to_bits()),
            ceiling: AtomicU32::new(0.99f32.to_bits()),
            release_ms: AtomicU32::new(50.0f32.to_bits()),
            lookahead_ms: AtomicU32::new(2.0f32.to_bits()),
            delay_left: [0.0; LOOKAHEAD_CAPACITY],
            delay_right: [0.0; LOOKAHEAD_CAPACITY],
            write_index: 0,
            current_gain: 1.0,
        };
        limiter.prepare(sample_rate);
        limiter
    }

    fn prepare(&mut self, sample_rate: f64) {
        self.sample_rate =
            if sample_rate.is_finite() && crate::dsp_limits::is_supported_sample_rate(sample_rate) {
                sample_rate
            } else {
                44_100.0
            };
        self.reset();
    }

    fn reset(&mut self) {
        self.delay_left.fill(0.0);
        self.delay_right.fill(0.0);
        self.write_index = 0;
        self.current_gain = 1.0;
    }

    fn process(&mut self, left: &mut [f32], right: Option<&mut [f32]>) {
        let frames = right
            .as_ref()
            .map_or(left.len(), |right| left.len().min(right.len()));
        let mut right = right.map(|right| &mut right[..frames]);
        let left = &mut left[..frames];
        let sample_rate = if self.sample_rate.is_finite() && self.sample_rate > 0.0 {
            self.sample_rate
        } else {
            44_100.0
        };
        let release_ms = f32::from_bits(self.release_ms.load(Ordering::Relaxed));
        let release_ms = if release_ms.is_finite() {
            release_ms.clamp(1.0, 1000.0)
        } else {
            50.0
        };
        let mut release_coefficient =
            (-1.0_f32 / (release_ms * 0.001_f32 * sample_rate as f32)).exp();
        if !release_coefficient.is_finite() {
            release_coefficient = 0.99;
        }
        release_coefficient = release_coefficient.clamp(0.0, 0.9999);
        let threshold_gain = f32::from_bits(self.threshold_gain.load(Ordering::Relaxed));
        let threshold_gain = if threshold_gain.is_finite() {
            threshold_gain.clamp(0.001, 15.8489)
        } else {
            1.0
        };
        let ceiling = f32::from_bits(self.ceiling.load(Ordering::Relaxed));
        let ceiling = if ceiling.is_finite() {
            ceiling.clamp(0.001, 1.0)
        } else {
            0.99
        };
        let lookahead_ms = f32::from_bits(self.lookahead_ms.load(Ordering::Relaxed));
        let lookahead_ms = if lookahead_ms.is_finite() {
            lookahead_ms.clamp(0.0, 20.0)
        } else {
            2.0
        };
        let lookahead_samples = ((lookahead_ms * 0.001_f32 * sample_rate as f32).round() as u32)
            .min((LOOKAHEAD_CAPACITY - 1) as u32) as usize;

        for frame in 0..frames {
            let input_left = if left[frame].is_finite() {
                left[frame] * threshold_gain
            } else {
                0.0
            };
            let input_right = match right.as_ref() {
                Some(right) if right[frame].is_finite() => right[frame] * threshold_gain,
                Some(_) => input_left,
                None => input_left,
            };

            self.delay_left[self.write_index] = input_left;
            self.delay_right[self.write_index] = input_right;
            let read_index =
                (self.write_index + LOOKAHEAD_CAPACITY - lookahead_samples) & LOOKAHEAD_MASK;
            let output_left = self.delay_left[read_index];
            let output_right = self.delay_right[read_index];

            let mut peak = 0.0_f32;
            for ahead in 0..=lookahead_samples {
                let index = (read_index + ahead) & LOOKAHEAD_MASK;
                let previous = (index + LOOKAHEAD_CAPACITY - 1) & LOOKAHEAD_MASK;
                peak = peak.max(
                    self.delay_left[index]
                        .abs()
                        .max(self.delay_right[index].abs()),
                );
                peak = peak.max(
                    (0.5 * (self.delay_left[previous] + self.delay_left[index]))
                        .abs()
                        .max((0.5 * (self.delay_right[previous] + self.delay_right[index])).abs()),
                );
            }
            self.write_index = (self.write_index + 1) & LOOKAHEAD_MASK;

            let mut target_attenuation = if peak > ceiling {
                ceiling / (peak + 1.0e-6)
            } else {
                1.0
            };
            if !target_attenuation.is_finite() {
                target_attenuation = 1.0;
            }
            if target_attenuation < self.current_gain {
                self.current_gain = target_attenuation;
            } else {
                self.current_gain = self.current_gain * release_coefficient
                    + target_attenuation * (1.0 - release_coefficient);
            }
            if !self.current_gain.is_finite() {
                self.current_gain = 1.0;
            }

            let limited_left = output_left * self.current_gain;
            left[frame] = if limited_left.is_finite() {
                limited_left.clamp(-ceiling, ceiling)
            } else {
                0.0
            };
            if let Some(right) = right.as_deref_mut() {
                let limited_right = output_right * self.current_gain;
                right[frame] = if limited_right.is_finite() {
                    limited_right.clamp(-ceiling, ceiling)
                } else {
                    0.0
                };
            }
        }
    }

    fn latency_samples(&self, lookahead_ms: f32) -> u32 {
        let lookahead_ms = if lookahead_ms.is_finite() {
            lookahead_ms.clamp(0.0, 20.0)
        } else {
            2.0
        };
        ((lookahead_ms as f64 * 0.001 * self.sample_rate).round() as u32)
            .min((LOOKAHEAD_CAPACITY - 1) as u32)
    }

    fn tail_samples(&self, release_ms: f32) -> u32 {
        let release_ms = if release_ms.is_finite() {
            release_ms.clamp(1.0, 1000.0)
        } else {
            50.0
        };
        (30.0 * self.sample_rate).min(release_ms as f64 * 0.001 * self.sample_rate * 8.0) as u32
    }

            }

#[no_mangle]
pub extern "C" fn hirari_master_limiter_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(MasterLimiter::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_limiter_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<MasterLimiter>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_limiter_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<MasterLimiter>().as_mut() } {
        state.prepare(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_limiter_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<MasterLimiter>().as_mut() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_limiter_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
    has_right: bool,
) {
    let Some(state) = (unsafe { state.cast::<MasterLimiter>().as_mut() }) else {
        return;
    };
    if left.is_null() || (has_right && right.is_null()) || frames > 65_536 {
        return;
    }
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames) };
    let right = if has_right {
        Some(unsafe { std::slice::from_raw_parts_mut(right, frames) })
    } else {
        None
    };
    state.process(left, right);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_limiter_latency(state: *const c_void) -> u32 {
    unsafe { state.cast::<MasterLimiter>().as_ref() }.map_or(0, |state| {
        state.latency_samples(f32::from_bits(state.lookahead_ms.load(Ordering::Relaxed)))
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_limiter_tail(state: *const c_void) -> u32 {
    unsafe { state.cast::<MasterLimiter>().as_ref() }.map_or(0, |state| {
        state.tail_samples(f32::from_bits(state.release_ms.load(Ordering::Relaxed)))
    })
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "dsp-differential-reference")]
    use super::MasterLimiter;
    #[cfg(feature = "dsp-differential-reference")]
    use std::sync::atomic::Ordering;

    #[cfg(feature = "dsp-differential-reference")]
    unsafe extern "C" {
        fn hirari_master_limiter_reference_create(sample_rate: f64) -> *mut std::ffi::c_void;
        fn hirari_master_limiter_reference_destroy(state: *mut std::ffi::c_void);
        fn hirari_master_limiter_reference_process(
            state: *mut std::ffi::c_void,
            left: *mut f32,
            right: *mut f32,
            frames: usize,
            has_right: bool,
            threshold_gain: f32,
            ceiling: f32,
            release_ms: f32,
            lookahead_ms: f32,
        );
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn realtime_limiter_matches_cpp_reference_across_blocks_and_settings() {
        let mut rust = MasterLimiter::new(48_000.0);
        let cpp = unsafe { hirari_master_limiter_reference_create(48_000.0) };
        assert!(!cpp.is_null());
        let mut rust_left = [0.0_f32; 2048];
        let mut rust_right = [0.0_f32; 2048];
        let mut cpp_left = [0.0_f32; 2048];
        let mut cpp_right = [0.0_f32; 2048];
        for block in 0..8 {
            let start = block * 256;
            let end = start + 256;
            for frame in start..end {
                let sample = if frame % 257 == 0 {
                    1.4
                } else {
                    (frame as f32 * 0.037).sin() * 0.73
                };
                rust_left[frame] = sample;
                cpp_left[frame] = sample;
                rust_right[frame] = -sample * 0.8;
                cpp_right[frame] = -sample * 0.8;
            }
            if block == 5 {
                rust_left[start + 4] = f32::INFINITY;
                cpp_left[start + 4] = f32::INFINITY;
                rust_right[start + 9] = f32::NAN;
                cpp_right[start + 9] = f32::NAN;
            }
            let (threshold, ceiling, release, lookahead): (f32, f32, f32, f32) = match block {
                0..=1 => (1.25, 0.99, 50.0, 2.0),
                2..=4 => (0.8, 0.8, 120.0, 0.0),
                _ => (1.5, 0.95, 8.0, 12.5),
            };
            rust.threshold_gain
                .store(threshold.to_bits(), Ordering::Relaxed);
            rust.ceiling.store(ceiling.to_bits(), Ordering::Relaxed);
            rust.release_ms.store(release.to_bits(), Ordering::Relaxed);
            rust.lookahead_ms
                .store(lookahead.to_bits(), Ordering::Relaxed);
            rust.process(
                &mut rust_left[start..end],
                Some(&mut rust_right[start..end]),
            );
            unsafe {
                hirari_master_limiter_reference_process(
                    cpp,
                    cpp_left[start..end].as_mut_ptr(),
                    cpp_right[start..end].as_mut_ptr(),
                    end - start,
                    true,
                    threshold,
                    ceiling,
                    release,
                    lookahead,
                );
            }
        }
        unsafe { hirari_master_limiter_reference_destroy(cpp) };
        for (rust, cpp) in rust_left.iter().zip(cpp_left) {
            assert!((rust - cpp).abs() <= 1.0e-6, "{rust} != {cpp}");
        }
        for (rust, cpp) in rust_right.iter().zip(cpp_right) {
            assert!((rust - cpp).abs() <= 1.0e-6, "{rust} != {cpp}");
        }
    }
}
