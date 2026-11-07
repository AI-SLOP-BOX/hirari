//! Per-Track delay rings for plugin delay compensation and manual timing offset.

use std::ffi::c_void;

const MAX_DELAY: u32 = 8192;
const CAPACITY: usize = MAX_DELAY as usize + 1;
const TRANSITION_SAMPLES: u32 = 64;

struct TrackPdcDelay {
    delay_l: [f32; CAPACITY],
    delay_r: [f32; CAPACITY],
    pre_delay_l: [f32; CAPACITY],
    pre_delay_r: [f32; CAPACITY],
    previous: u32,
    active: u32,
    transition_remaining: u32,
    write: usize,
}

impl TrackPdcDelay {
    fn new() -> Self {
        Self {
            delay_l: [0.0; CAPACITY],
            delay_r: [0.0; CAPACITY],
            pre_delay_l: [0.0; CAPACITY],
            pre_delay_r: [0.0; CAPACITY],
            previous: 0,
            active: 0,
            transition_remaining: 0,
            write: 0,
        }
    }

    fn reset(&mut self, requested: u32) {
        self.delay_l.fill(0.0);
        self.delay_r.fill(0.0);
        self.pre_delay_l.fill(0.0);
        self.pre_delay_r.fill(0.0);
        self.write = 0;
        self.active = requested.min(MAX_DELAY);
        self.previous = self.active;
        self.transition_remaining = 0;
    }

    fn delayed(&self, delay: u32, right: bool, pre_tap: bool) -> f32 {
        if delay == 0 {
            return 0.0;
        }
        let read = (self.write + CAPACITY - delay as usize) % CAPACITY;
        match (pre_tap, right) {
            (true, false) => self.pre_delay_l[read],
            (true, true) => self.pre_delay_r[read],
            (false, false) => self.delay_l[read],
            (false, true) => self.delay_r[read],
        }
    }

    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        mut pre_left: Option<&mut [f32]>,
        mut pre_right: Option<&mut [f32]>,
        requested: u32,
    ) {
        let requested = requested.min(MAX_DELAY);
        if requested != self.active {
            self.previous = self.active;
            self.active = requested;
            self.transition_remaining = TRANSITION_SAMPLES;
        }

        for index in 0..left.len().min(right.len()) {
            let input_l = left[index];
            let input_r = right[index];
            let pre_input_l = pre_left.as_ref().map_or(0.0, |samples| samples[index]);
            let pre_input_r = pre_right.as_ref().map_or(0.0, |samples| samples[index]);
            let old_l = if self.previous == 0 {
                input_l
            } else {
                self.delayed(self.previous, false, false)
            };
            let old_r = if self.previous == 0 {
                input_r
            } else {
                self.delayed(self.previous, true, false)
            };
            let new_l = if self.active == 0 {
                input_l
            } else {
                self.delayed(self.active, false, false)
            };
            let new_r = if self.active == 0 {
                input_r
            } else {
                self.delayed(self.active, true, false)
            };
            let old_pre_l = if self.previous == 0 {
                pre_input_l
            } else {
                self.delayed(self.previous, false, true)
            };
            let old_pre_r = if self.previous == 0 {
                pre_input_r
            } else {
                self.delayed(self.previous, true, true)
            };
            let new_pre_l = if self.active == 0 {
                pre_input_l
            } else {
                self.delayed(self.active, false, true)
            };
            let new_pre_r = if self.active == 0 {
                pre_input_r
            } else {
                self.delayed(self.active, true, true)
            };

            if self.transition_remaining > 0 {
                let progress = (TRANSITION_SAMPLES - self.transition_remaining) as f32
                    / (TRANSITION_SAMPLES - 1) as f32;
                left[index] = old_l + (new_l - old_l) * progress;
                right[index] = old_r + (new_r - old_r) * progress;
                if let Some(samples) = pre_left.as_deref_mut() {
                    samples[index] = old_pre_l + (new_pre_l - old_pre_l) * progress;
                }
                if let Some(samples) = pre_right.as_deref_mut() {
                    samples[index] = old_pre_r + (new_pre_r - old_pre_r) * progress;
                }
                self.transition_remaining -= 1;
            } else {
                left[index] = new_l;
                right[index] = new_r;
                if let Some(samples) = pre_left.as_deref_mut() {
                    samples[index] = new_pre_l;
                }
                if let Some(samples) = pre_right.as_deref_mut() {
                    samples[index] = new_pre_r;
                }
            }

            self.delay_l[self.write] = if input_l.is_finite() { input_l } else { 0.0 };
            self.delay_r[self.write] = if input_r.is_finite() { input_r } else { 0.0 };
            self.pre_delay_l[self.write] = if pre_input_l.is_finite() {
                pre_input_l
            } else {
                0.0
            };
            self.pre_delay_r[self.write] = if pre_input_r.is_finite() {
                pre_input_r
            } else {
                0.0
            };
            self.write = (self.write + 1) % CAPACITY;
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_track_pdc_delay_create() -> *mut c_void {
    Box::into_raw(Box::new(TrackPdcDelay::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_pdc_delay_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<TrackPdcDelay>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_pdc_delay_reset(state: *mut c_void, requested: u32) {
    if let Some(state) = unsafe { state.cast::<TrackPdcDelay>().as_mut() } {
        state.reset(requested);
    }
}

/// # Safety
/// `left` and `right` must each reference `frames` writable floats. Optional
/// pre-fader pointers must either both be null or each reference `frames`
/// writable floats. `state` must be a live TrackPdcDelay handle.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_pdc_delay_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    pre_left: *mut f32,
    pre_right: *mut f32,
    frames: u32,
    requested: u32,
) {
    let Some(state) = (unsafe { state.cast::<TrackPdcDelay>().as_mut() }) else {
        return;
    };
    if frames == 0 || left.is_null() || right.is_null() {
        return;
    }
    let frames = frames as usize;
    let (pre_left, pre_right) = if pre_left.is_null() || pre_right.is_null() {
        (None, None)
    } else {
        unsafe {
            (
                Some(std::slice::from_raw_parts_mut(pre_left, frames)),
                Some(std::slice::from_raw_parts_mut(pre_right, frames)),
            )
        }
    };
    unsafe {
        state.process(
            std::slice::from_raw_parts_mut(left, frames),
            std::slice::from_raw_parts_mut(right, frames),
            pre_left,
            pre_right,
            requested,
        );
    }
}

#[cfg(all(test, feature = "dsp-differential-reference"))]
mod differential_tests {
    use super::*;

    unsafe extern "C" {
        fn hirari_track_pdc_delay_process_reference(
            state: *mut c_void,
            left: *mut f32,
            right: *mut f32,
            pre_left: *mut f32,
            pre_right: *mut f32,
            frames: u32,
            requested: u32,
        );
        fn hirari_track_pdc_delay_reset_reference(state: *mut c_void, requested: u32);
        fn hirari_track_pdc_delay_create_reference() -> *mut c_void;
        fn hirari_track_pdc_delay_destroy_reference(state: *mut c_void);
    }

    struct CppReference(*mut c_void);
    impl CppReference {
        fn new() -> Self {
            Self(unsafe { hirari_track_pdc_delay_create_reference() })
        }
        fn reset(&mut self, requested: u32) {
            unsafe { hirari_track_pdc_delay_reset_reference(self.0, requested) }
        }
        fn process(
            &mut self,
            left: &mut [f32],
            right: &mut [f32],
            pre_left: Option<&mut [f32]>,
            pre_right: Option<&mut [f32]>,
            requested: u32,
        ) {
            let (pre_left, pre_right) = match (pre_left, pre_right) {
                (Some(left), Some(right)) => (left.as_mut_ptr(), right.as_mut_ptr()),
                _ => (std::ptr::null_mut(), std::ptr::null_mut()),
            };
            unsafe {
                hirari_track_pdc_delay_process_reference(
                    self.0,
                    left.as_mut_ptr(),
                    right.as_mut_ptr(),
                    pre_left,
                    pre_right,
                    left.len() as u32,
                    requested,
                );
            }
        }
    }
    impl Drop for CppReference {
        fn drop(&mut self) {
            unsafe { hirari_track_pdc_delay_destroy_reference(self.0) }
        }
    }

    #[test]
    fn pdc_delay_matches_cpp_for_live_taps_transitions_and_pre_fader_mirror() {
        let mut rust = TrackPdcDelay::new();
        let mut cpp = CppReference::new();
        let mut playhead = 0usize;
        for (block, frames, requested) in [
            (0usize, 1usize, 0u32),
            (1, 63, 0),
            (2, 257, 31),
            (3, 128, 31),
            (4, 17, 7),
            (5, 512, 129),
            (6, 4, MAX_DELAY),
            (7, 8500, MAX_DELAY + 100),
            (8, 73, 0),
            (9, 1024, 3),
        ] {
            let make_signal = |channel: usize| {
                (0..frames)
                    .map(|index| {
                        let position = (playhead + index + block * 19 + channel * 71) as f32;
                        (position * (0.019 + channel as f32 * 0.011)).sin()
                            * (0.25 + channel as f32 * 0.3)
                    })
                    .collect::<Vec<_>>()
            };
            let mut rust_l = make_signal(0);
            let mut rust_r = make_signal(1);
            let mut rust_pre_l = make_signal(2);
            let mut rust_pre_r = make_signal(3);
            let (mut cpp_l, mut cpp_r, mut cpp_pre_l, mut cpp_pre_r) = (
                rust_l.clone(),
                rust_r.clone(),
                rust_pre_l.clone(),
                rust_pre_r.clone(),
            );
            if block == 4 {
                rust_l[0] = f32::NAN;
                rust_r[0] = f32::INFINITY;
                cpp_l[0] = f32::NAN;
                cpp_r[0] = f32::INFINITY;
            }
            rust.process(
                &mut rust_l,
                &mut rust_r,
                Some(&mut rust_pre_l),
                Some(&mut rust_pre_r),
                requested,
            );
            cpp.process(
                &mut cpp_l,
                &mut cpp_r,
                Some(&mut cpp_pre_l),
                Some(&mut cpp_pre_r),
                requested,
            );
            for (rust, cpp) in [
                (&rust_l, &cpp_l),
                (&rust_r, &cpp_r),
                (&rust_pre_l, &cpp_pre_l),
                (&rust_pre_r, &cpp_pre_r),
            ] {
                for (&rust, &cpp) in rust.iter().zip(cpp) {
                    assert!(rust == cpp || (rust - cpp).abs() < 1.0e-6);
                }
            }
            playhead += frames;
        }
        rust.reset(45);
        cpp.reset(45);
        for _ in 0..3 {
            let mut rust_l = [1.0; 96];
            let mut rust_r = [-0.5; 96];
            let mut cpp_l = rust_l;
            let mut cpp_r = rust_r;
            rust.process(&mut rust_l, &mut rust_r, None, None, 45);
            cpp.process(&mut cpp_l, &mut cpp_r, None, None, 45);
            assert_eq!(rust_l, cpp_l);
            assert_eq!(rust_r, cpp_r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_clears_history_and_clamps_requested_delay() {
        let mut state = TrackPdcDelay::new();
        state.active = 5;
        state.previous = 4;
        state.transition_remaining = 20;
        state.delay_l[0] = 0.5;
        state.reset(u32::MAX);
        assert_eq!(state.active, MAX_DELAY);
        assert_eq!(state.previous, MAX_DELAY);
        assert_eq!(state.transition_remaining, 0);
        assert_eq!(state.delay_l[0], 0.0);
    }
}
