use std::ffi::c_void;

const DELAY_BASE_SAMPLES: [usize; 8] = [1117, 1373, 1601, 2111, 2711, 3121, 3701, 4127];

pub struct LushReverbEngine {
    pub sample_rate: f64,
    pub delay_lines: [Vec<f32>; 8],
    pub write_indices: [usize; 8],
    pub filter_state: [f32; 8],
    pub feedback: f32,
    pub damping: f32,
}

impl LushReverbEngine {
    pub fn new(sample_rate: f64) -> Self {
        let mut engine = Self {
            sample_rate: 44_100.0,
            delay_lines: std::array::from_fn(|_| Vec::new()),
            write_indices: [0; 8],
            filter_state: [0.0; 8],
            feedback: 0.85,
            damping: 0.2,
        };
        engine.set_sample_rate(sample_rate);
        engine
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate =
            if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
                sample_rate
            } else {
                44_100.0
            };
        let scale = self.sample_rate / 44_100.0;
        for (line, base) in self.delay_lines.iter_mut().zip(DELAY_BASE_SAMPLES) {
            let length = ((base as f64 * scale).round() as usize).max(1);
            line.resize(length, 0.0);
            line.fill(0.0);
        }
        self.write_indices = [0; 8];
    }

    pub fn set_feedback(&mut self, feedback: f32) {
        self.feedback = if feedback.is_finite() {
            feedback.clamp(0.0, 0.995)
        } else {
            0.85
        };
    }

    pub fn set_damping(&mut self, damping: f32) {
        self.damping = if damping.is_finite() {
            damping.clamp(0.0, 0.99)
        } else {
            0.2
        };
    }

    pub fn reset(&mut self) {
        for line in &mut self.delay_lines {
            line.fill(0.0);
        }
        self.write_indices = [0; 8];
        self.filter_state = [0.0; 8];
    }

    fn process_frame(&mut self, in_l: f32, in_r: f32, feedback: f32, damping: f32) -> (f32, f32) {
        let input = 0.5 * (in_l + in_r);
        let mut sum = 0.0f32;

        for i in 0..8 {
            let line = &self.delay_lines[i];
            let read = (self.write_indices[i] + 1) % line.len();
            let delayed = if line[read].is_finite() {
                line[read]
            } else {
                0.0
            };
            self.filter_state[i] += (delayed - self.filter_state[i]) * (1.0 - damping);
            sum += self.filter_state[i];
        }

        let mean = sum / 8.0;
        let mut wet_l = 0.0f32;
        let mut wet_r = 0.0f32;
        for i in 0..8 {
            let diffuse = self.filter_state[i] - 2.0 * mean;
            let next = input + feedback * diffuse;
            let line = &mut self.delay_lines[i];
            line[self.write_indices[i]] = if next.is_finite() { next } else { 0.0 };
            self.write_indices[i] = (self.write_indices[i] + 1) % line.len();
            if i & 1 == 0 {
                wet_l += self.filter_state[i];
            } else {
                wet_r += self.filter_state[i];
            }
        }

        let out_l = in_l * 0.75 + wet_l * 0.03125;
        let out_r = in_r * 0.75 + wet_r * 0.03125;
        (
            if out_l.is_finite() {
                out_l.clamp(-16.0, 16.0)
            } else {
                0.0
            },
            if out_r.is_finite() {
                out_r.clamp(-16.0, 16.0)
            } else {
                0.0
            },
        )
    }

    pub fn process_stereo(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len());
        if frames == 0 || self.delay_lines.iter().any(Vec::is_empty) {
            return;
        }
        let feedback = self.feedback.clamp(0.0, 0.995);
        let damping = self.damping.clamp(0.0, 0.99);
        for frame in 0..frames {
            let in_l = if left[frame].is_finite() {
                left[frame]
            } else {
                0.0
            };
            let in_r = if right[frame].is_finite() {
                right[frame]
            } else {
                0.0
            };
            let (out_l, out_r) = self.process_frame(in_l, in_r, feedback, damping);
            left[frame] = out_l;
            right[frame] = out_r;
        }
    }

    pub fn process_mono(&mut self, samples: &mut [f32]) {
        if samples.is_empty() || self.delay_lines.iter().any(Vec::is_empty) {
            return;
        }
        let feedback = self.feedback.clamp(0.0, 0.995);
        let damping = self.damping.clamp(0.0, 0.99);
        for sample in samples {
            let input = if sample.is_finite() { *sample } else { 0.0 };
            let (out_l, out_r) = self.process_frame(input, input, feedback, damping);
            let mono = 0.5 * (out_l + out_r);
            *sample = if mono.is_finite() {
                mono.clamp(-16.0, 16.0)
            } else {
                0.0
            };
        }
    }

    pub fn tail_samples(&self) -> u32 {
        let longest = self.delay_lines.iter().map(Vec::len).max().unwrap_or(0);
        let practical_tail = longest.saturating_mul(48);
        let maximum_tail = (self.sample_rate * 30.0) as usize;
        practical_tail.min(maximum_tail) as u32
    }

    pub fn audit_lush_reverb(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.feedback.is_finite()
            && (0.0..=0.995).contains(&self.feedback)
            && self.damping.is_finite()
            && (0.0..=0.99).contains(&self.damping)
            && self
                .delay_lines
                .iter()
                .enumerate()
                .all(|(i, line)| !line.is_empty() && self.write_indices[i] < line.len())
            && self.filter_state.iter().all(|value| value.is_finite())
    }
}

#[no_mangle]
pub extern "C" fn hirari_lush_reverb_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(LushReverbEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_lush_reverb_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: the handle was allocated by `hirari_lush_reverb_create`.
        unsafe { drop(Box::from_raw(state.cast::<LushReverbEngine>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_lush_reverb_set_sample_rate(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<LushReverbEngine>().as_mut() } {
        state.set_sample_rate(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_lush_reverb_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<LushReverbEngine>().as_mut() } {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_lush_reverb_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) {
    if left.is_null() || right.is_null() {
        return;
    }
    let Some(state) = (unsafe { state.cast::<LushReverbEngine>().as_mut() }) else {
        return;
    };
    if left == right {
        // SAFETY: the host provides a writable mono buffer of `frames` samples.
        state.process_mono(unsafe { std::slice::from_raw_parts_mut(left, frames) });
    } else {
        // SAFETY: the host provides two non-overlapping writable buffers.
        let (left, right) = unsafe {
            (
                std::slice::from_raw_parts_mut(left, frames),
                std::slice::from_raw_parts_mut(right, frames),
            )
        };
        state.process_stereo(left, right);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_lush_reverb_tail(state: *const c_void) -> u32 {
    unsafe { state.cast::<LushReverbEngine>().as_ref() }.map_or(0, LushReverbEngine::tail_samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CppReference {
        delay_lines: [Vec<f32>; 8],
        write_indices: [usize; 8],
        filter_state: [f32; 8],
        feedback: f32,
        damping: f32,
    }

    impl CppReference {
        fn new(sample_rate: f64) -> Self {
            let bases = [1117, 1373, 1601, 2111, 2711, 3121, 3701, 4127];
            let scale = sample_rate / 44_100.0;
            Self {
                delay_lines: std::array::from_fn(|i| {
                    vec![0.0; (bases[i] as f64 * scale).round() as usize]
                }),
                write_indices: [0; 8],
                filter_state: [0.0; 8],
                feedback: 0.85,
                damping: 0.2,
            }
        }

        fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
            for frame in 0..left.len().min(right.len()) {
                let in_l = if left[frame].is_finite() {
                    left[frame]
                } else {
                    0.0
                };
                let in_r = if right[frame].is_finite() {
                    right[frame]
                } else {
                    0.0
                };
                let input = 0.5 * (in_l + in_r);
                let mut sum = 0.0f32;
                for i in 0..8 {
                    let line = &self.delay_lines[i];
                    let read = (self.write_indices[i] + 1) % line.len();
                    let delayed = if line[read].is_finite() {
                        line[read]
                    } else {
                        0.0
                    };
                    self.filter_state[i] += (delayed - self.filter_state[i]) * (1.0 - self.damping);
                    sum += self.filter_state[i];
                }
                let mean = sum / 8.0;
                let mut wet_l = 0.0f32;
                let mut wet_r = 0.0f32;
                for i in 0..8 {
                    let diffuse = self.filter_state[i] - 2.0 * mean;
                    let next = input + self.feedback * diffuse;
                    let line = &mut self.delay_lines[i];
                    line[self.write_indices[i]] = if next.is_finite() { next } else { 0.0 };
                    self.write_indices[i] = (self.write_indices[i] + 1) % line.len();
                    if i & 1 == 0 {
                        wet_l += self.filter_state[i];
                    } else {
                        wet_r += self.filter_state[i];
                    }
                }
                let out_l = in_l * 0.75 + wet_l * 0.03125;
                let out_r = in_r * 0.75 + wet_r * 0.03125;
                left[frame] = if out_l.is_finite() {
                    out_l.clamp(-16.0, 16.0)
                } else {
                    0.0
                };
                right[frame] = if out_r.is_finite() {
                    out_r.clamp(-16.0, 16.0)
                } else {
                    0.0
                };
            }
        }
    }

    #[test]
    fn rust_port_matches_the_cpp_reverb_for_impulse_and_deterministic_audio() {
        for sample_rate in [44_100.0, 48_000.0, 96_000.0] {
            let mut rust = LushReverbEngine::new(sample_rate);
            let mut cpp = CppReference::new(sample_rate);
            let mut rust_left = vec![0.0; 16_000];
            let mut rust_right = vec![0.0; 16_000];
            rust_left[0] = 1.0;
            rust_right[0] = -0.75;
            for (frame, (left, right)) in rust_left
                .iter_mut()
                .zip(&mut rust_right)
                .enumerate()
                .skip(1)
            {
                *left = ((frame as f32 * 0.017).sin() * 0.2)
                    + ((frame * 17 % 101) as f32 / 101.0 - 0.5) * 0.03;
                *right = ((frame as f32 * 0.023).cos() * 0.17)
                    + ((frame * 29 % 97) as f32 / 97.0 - 0.5) * 0.02;
            }
            let mut cpp_left = rust_left.clone();
            let mut cpp_right = rust_right.clone();
            for start in (0..rust_left.len()).step_by(257) {
                let end = (start + 257).min(rust_left.len());
                rust.process_stereo(&mut rust_left[start..end], &mut rust_right[start..end]);
                cpp.process(&mut cpp_left[start..end], &mut cpp_right[start..end]);
            }
            assert_eq!(rust_left, cpp_left);
            assert_eq!(rust_right, cpp_right);
        }
    }

    #[test]
    fn mono_process_and_tail_follow_the_cpp_contract() {
        let mut engine = LushReverbEngine::new(48_000.0);
        let expected_longest = (4127.0_f64 * (48_000.0 / 44_100.0)).round() as usize;
        assert_eq!(engine.tail_samples(), (expected_longest * 48) as u32);
        let mut mono = vec![0.0; 4096];
        mono[0] = 1.0;
        engine.process_mono(&mut mono);
        assert!(mono.iter().all(|sample| sample.is_finite()));
        assert!(mono.iter().any(|sample| *sample != 0.0));
    }

    #[test]
    fn ffi_mono_buffer_path_handles_aliasing_without_creating_two_mutable_slices() {
        let state = hirari_lush_reverb_create(48_000.0);
        assert!(!state.is_null());
        let mut actual = vec![0.0; 5000];
        actual[0] = 1.0;
        actual[337] = -0.25;
        let mut expected = actual.clone();
        LushReverbEngine::new(48_000.0).process_mono(&mut expected);
        let frames = actual.len();
        unsafe {
            hirari_lush_reverb_process(state, actual.as_mut_ptr(), actual.as_mut_ptr(), frames);
        }
        assert_eq!(actual, expected);
        unsafe { hirari_lush_reverb_destroy(state) };
    }
}
