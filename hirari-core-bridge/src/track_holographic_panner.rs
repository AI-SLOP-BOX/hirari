//! Stateful Track spatial panning and bounded HRTF convolution.

use arc_swap::ArcSwap;
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

const HRTF_TAPS: usize = 128;

#[derive(Clone)]
struct PannerConfig {
    sample_rate: f64,
    taps: usize,
    hrtf_l: [f32; HRTF_TAPS],
    hrtf_r: [f32; HRTF_TAPS],
    reset_generation: u64,
    reset_delay_generation: u64,
}

impl Default for PannerConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            taps: 0,
            hrtf_l: [0.0; HRTF_TAPS],
            hrtf_r: [0.0; HRTF_TAPS],
            reset_generation: 0,
            reset_delay_generation: 0,
        }
    }
}

struct RealtimePanner {
    delay_l: [f32; 512],
    delay_r: [f32; 512],
    history: [f32; HRTF_TAPS],
    history_ptr: u32,
    ptr: u32,
    reset_generation: u64,
    reset_delay_generation: u64,
}

impl Default for RealtimePanner {
    fn default() -> Self {
        Self {
            delay_l: [0.0; 512],
            delay_r: [0.0; 512],
            history: [0.0; HRTF_TAPS],
            history_ptr: 0,
            ptr: 0,
            reset_generation: 0,
            reset_delay_generation: 0,
        }
    }
}

struct NativePanner {
    config: ArcSwap<PannerConfig>,
    control_update: Mutex<()>,
    realtime: UnsafeCell<RealtimePanner>,
}

// The mutable realtime state is only accessed by the serialized audio callback.
unsafe impl Sync for NativePanner {}

impl NativePanner {
    fn new() -> Self {
        Self {
            config: ArcSwap::from_pointee(PannerConfig::default()),
            control_update: Mutex::new(()),
            realtime: UnsafeCell::new(RealtimePanner::default()),
        }
    }

    fn update_config(&self, update: impl FnOnce(&mut PannerConfig)) {
        let _guard = self
            .control_update
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut next = (*self.config.load_full()).clone();
        update(&mut next);
        self.config.store(Arc::new(next));
    }

    fn set_sample_rate(&self, sample_rate: f64) {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return;
        }
        self.update_config(|config| {
            config.sample_rate = sample_rate;
            config.reset_generation = config.reset_generation.wrapping_add(1);
            config.reset_delay_generation = config.reset_generation;
        });
    }

    fn set_kernel(&self, left: &[f32], right: &[f32]) -> bool {
        if left.is_empty()
            || left.len() != right.len()
            || left.len() > HRTF_TAPS
            || left.iter().chain(right).any(|sample| !sample.is_finite())
        {
            return false;
        }
        self.update_config(|config| {
            config.hrtf_l.fill(0.0);
            config.hrtf_r.fill(0.0);
            config.hrtf_l[..left.len()].copy_from_slice(left);
            config.hrtf_r[..right.len()].copy_from_slice(right);
            config.taps = left.len();
            config.reset_generation = config.reset_generation.wrapping_add(1);
        });
        true
    }

    fn clear_kernel(&self) {
        self.update_config(|config| {
            config.taps = 0;
            config.hrtf_l.fill(0.0);
            config.hrtf_r.fill(0.0);
        });
    }

    fn process(&self, left: &mut [f32], right: &mut [f32], x: f32, y: f32, z: f32) {
        let config = self.config.load();
        // SAFETY: Track processing serializes calls into its per-Track panner.
        let realtime = unsafe { &mut *self.realtime.get() };
        if realtime.reset_generation != config.reset_generation {
            realtime.history.fill(0.0);
            realtime.history_ptr = 0;
            realtime.reset_generation = config.reset_generation;
        }
        if realtime.reset_delay_generation != config.reset_delay_generation {
            realtime.delay_l.fill(0.0);
            realtime.delay_r.fill(0.0);
            realtime.ptr = 0;
            realtime.reset_delay_generation = config.reset_delay_generation;
        }

        let x = if x.is_finite() {
            x.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let y = if y.is_finite() {
            y.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let z = if z.is_finite() {
            z.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let sample_rate = if config.sample_rate.is_finite() && config.sample_rate > 0.0 {
            config.sample_rate
        } else {
            48_000.0
        };
        let distance = (1.0 - 0.35 * 0.0_f32.max(z) - 0.15 * y.abs()).clamp(0.25, 1.0);
        let angle = (x + 1.0) * 0.25 * std::f32::consts::PI;
        let left_gain = angle.cos() * distance;
        let right_gain = angle.sin() * distance;
        let delay_samples = ((x.abs() as f64 * 0.0007 * sample_rate) as u32).min(511);

        for (in_l, in_r) in left.iter_mut().zip(right.iter_mut()) {
            let input_l = *in_l;
            let input_r = *in_r;
            let mono = 0.5
                * (if input_l.is_finite() { input_l } else { 0.0 }
                    + if input_r.is_finite() { input_r } else { 0.0 });
            let slot = realtime.ptr % 512;
            realtime.ptr = realtime.ptr.wrapping_add(1);
            realtime.delay_l[slot as usize] = mono;
            realtime.delay_r[slot as usize] = mono;
            let delayed = (slot + 512 - delay_samples) % 512;
            if config.taps != 0 {
                let history_slot = realtime.history_ptr % HRTF_TAPS as u32;
                realtime.history[history_slot as usize] = mono;
                realtime.history_ptr = realtime.history_ptr.wrapping_add(1);
                let mut out_l = 0.0;
                let mut out_r = 0.0;
                for tap in 0..config.taps {
                    let index = (realtime.history_ptr + HRTF_TAPS as u32 - 1 - tap as u32)
                        % HRTF_TAPS as u32;
                    let sample = realtime.history[index as usize];
                    out_l += sample * config.hrtf_l[tap];
                    out_r += sample * config.hrtf_r[tap];
                }
                *in_l = if out_l.is_finite() {
                    out_l * distance
                } else {
                    0.0
                };
                *in_r = if out_r.is_finite() {
                    out_r * distance
                } else {
                    0.0
                };
            } else {
                let value_l = realtime.delay_l[delayed as usize] * left_gain;
                let value_r = realtime.delay_r[delayed as usize] * right_gain;
                *in_l = if value_l.is_finite() { value_l } else { 0.0 };
                *in_r = if value_r.is_finite() { value_r } else { 0.0 };
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_track_holographic_panner_create() -> *mut c_void {
    Box::into_raw(Box::new(NativePanner::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_holographic_panner_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<NativePanner>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_holographic_panner_set_sample_rate(
    state: *const c_void,
    sample_rate: f64,
) {
    if let Some(state) = unsafe { state.cast::<NativePanner>().as_ref() } {
        state.set_sample_rate(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_holographic_panner_sample_rate(state: *const c_void) -> f64 {
    unsafe { state.cast::<NativePanner>().as_ref() }
        .map(|state| state.config.load().sample_rate)
        .unwrap_or(48_000.0)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_holographic_panner_set_kernel(
    state: *const c_void,
    left: *const f32,
    right: *const f32,
    taps: u32,
) -> bool {
    let Some(state) = (unsafe { state.cast::<NativePanner>().as_ref() }) else {
        return false;
    };
    if left.is_null() || right.is_null() || taps == 0 || taps as usize > HRTF_TAPS {
        return false;
    }
    let left = unsafe { std::slice::from_raw_parts(left, taps as usize) };
    let right = unsafe { std::slice::from_raw_parts(right, taps as usize) };
    state.set_kernel(left, right)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_holographic_panner_clear_kernel(state: *const c_void) {
    if let Some(state) = unsafe { state.cast::<NativePanner>().as_ref() } {
        state.clear_kernel();
    }
}

/// # Safety
/// `left` and `right` must be disjoint writable arrays of `frames` floats.
/// Calls for a given state must be serialized with each other; control updates
/// may run concurrently and are published as immutable snapshots.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_holographic_panner_process(
    state: *const c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    x: f32,
    y: f32,
    z: f32,
) {
    let Some(state) = (unsafe { state.cast::<NativePanner>().as_ref() }) else {
        return;
    };
    if frames == 0 || left.is_null() || right.is_null() {
        return;
    }
    unsafe {
        state.process(
            std::slice::from_raw_parts_mut(left, frames as usize),
            std::slice::from_raw_parts_mut(right, frames as usize),
            x,
            y,
            z,
        );
    }
}

#[cfg(all(test, feature = "dsp-differential-reference"))]
mod differential_tests {
    use super::*;

    unsafe extern "C" {
        fn hirari_track_holographic_panner_create_reference() -> *mut c_void;
        fn hirari_track_holographic_panner_destroy_reference(state: *mut c_void);
        fn hirari_track_holographic_panner_set_sample_rate_reference(state: *mut c_void, rate: f64);
        fn hirari_track_holographic_panner_set_kernel_reference(
            state: *mut c_void,
            left: *const f32,
            right: *const f32,
            taps: u32,
        ) -> bool;
        fn hirari_track_holographic_panner_clear_kernel_reference(state: *mut c_void);
        fn hirari_track_holographic_panner_process_reference(
            state: *mut c_void,
            left: *mut f32,
            right: *mut f32,
            frames: u32,
            x: f32,
            y: f32,
            z: f32,
        );
    }

    struct CppReference(*mut c_void);
    impl CppReference {
        fn new() -> Self {
            Self(unsafe { hirari_track_holographic_panner_create_reference() })
        }
        fn process(&mut self, left: &mut [f32], right: &mut [f32], position: [f32; 3]) {
            unsafe {
                hirari_track_holographic_panner_process_reference(
                    self.0,
                    left.as_mut_ptr(),
                    right.as_mut_ptr(),
                    left.len() as u32,
                    position[0],
                    position[1],
                    position[2],
                )
            }
        }
        fn set_kernel(&mut self, left: &[f32], right: &[f32]) -> bool {
            unsafe {
                hirari_track_holographic_panner_set_kernel_reference(
                    self.0,
                    left.as_ptr(),
                    right.as_ptr(),
                    left.len() as u32,
                )
            }
        }
    }
    impl Drop for CppReference {
        fn drop(&mut self) {
            unsafe { hirari_track_holographic_panner_destroy_reference(self.0) }
        }
    }

    fn process_pair(
        rust: &NativePanner,
        cpp: &mut CppReference,
        left: &[f32],
        right: &[f32],
        position: [f32; 3],
    ) {
        let mut rust_left = left.to_vec();
        let mut rust_right = right.to_vec();
        let mut cpp_left = rust_left.clone();
        let mut cpp_right = rust_right.clone();
        rust.process(
            &mut rust_left,
            &mut rust_right,
            position[0],
            position[1],
            position[2],
        );
        cpp.process(&mut cpp_left, &mut cpp_right, position);
        for (&actual, &expected) in rust_left
            .iter()
            .zip(&cpp_left)
            .chain(rust_right.iter().zip(&cpp_right))
        {
            let tolerance = 2.0e-6 + 2.0e-6 * actual.abs().max(expected.abs());
            assert!(
                actual == expected || (actual - expected).abs() <= tolerance,
                "Rust={actual}, C++={expected}, delta={}",
                (actual - expected).abs()
            );
        }
    }

    #[test]
    fn panner_matches_cpp_across_modes_kernel_updates_and_sample_rates() {
        let rust = NativePanner::new();
        let mut cpp = CppReference::new();
        let make_block = |start: usize, frames: usize| {
            let left = (0..frames)
                .map(|i| (((start + i) as f32 * 0.013).sin()) * 0.8)
                .collect::<Vec<_>>();
            let right = (0..frames)
                .map(|i| (((start + i) as f32 * 0.021).cos()) * 0.6)
                .collect::<Vec<_>>();
            (left, right)
        };
        let positions = [
            [0.0, 0.0, 0.0],
            [-1.0, 1.0, 1.0],
            [0.75, -0.8, 0.6],
            [f32::NAN, f32::INFINITY, -f32::INFINITY],
        ];
        let mut offset = 0;
        for (block, frames) in [1usize, 64, 511, 17, 1024].into_iter().enumerate() {
            let (left, right) = make_block(offset, frames);
            process_pair(
                &rust,
                &mut cpp,
                &left,
                &right,
                positions[block % positions.len()],
            );
            offset += frames;
        }

        let mut ir_l = [0.0; HRTF_TAPS];
        let mut ir_r = [0.0; HRTF_TAPS];
        ir_l[0] = 0.75;
        ir_l[7] = -0.125;
        ir_r[0] = 0.5;
        ir_r[13] = 0.25;
        assert!(rust.set_kernel(&ir_l, &ir_r));
        assert!(cpp.set_kernel(&ir_l, &ir_r));
        for frames in [31usize, 128, 256] {
            let (left, right) = make_block(offset, frames);
            process_pair(&rust, &mut cpp, &left, &right, [0.2, 0.4, -0.3]);
            offset += frames;
        }

        let ir_short_l = [0.5, 0.25, -0.125];
        let ir_short_r = [-0.25, 0.75, 0.125];
        assert!(rust.set_kernel(&ir_short_l, &ir_short_r));
        assert!(cpp.set_kernel(&ir_short_l, &ir_short_r));
        let (left, right) = make_block(offset, 90);
        process_pair(&rust, &mut cpp, &left, &right, [0.9, -0.5, 0.2]);
        offset += 90;

        rust.clear_kernel();
        unsafe { hirari_track_holographic_panner_clear_kernel_reference(cpp.0) };
        let (left, right) = make_block(offset, 1200);
        process_pair(&rust, &mut cpp, &left, &right, [-0.7, 0.2, 0.8]);
        offset += 1200;

        rust.set_sample_rate(96_000.0);
        unsafe { hirari_track_holographic_panner_set_sample_rate_reference(cpp.0, 96_000.0) };
        let (left, right) = make_block(offset, 800);
        process_pair(&rust, &mut cpp, &left, &right, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn invalid_hrtf_kernels_leave_the_active_kernel_unchanged() {
        let rust = NativePanner::new();
        let mut cpp = CppReference::new();
        let left = [0.5, f32::NAN];
        let right = [0.25, 0.0];
        assert!(!rust.set_kernel(&left, &right));
        assert!(!cpp.set_kernel(&left, &right));
        let input_l = [1.0; 33];
        let input_r = [-0.5; 33];
        process_pair(&rust, &mut cpp, &input_l, &input_r, [0.4, 0.1, 0.3]);
    }
}
