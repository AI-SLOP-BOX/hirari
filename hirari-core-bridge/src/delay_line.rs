use std::ffi::c_void;

pub struct DelayLineEngine {
    pub buffer: Vec<f32>,
    pub write_idx: usize,
    pub mask: usize,
}

/// Integer-sample circular delay preserving the native delay-line contract.
struct IntegerDelayLine {
    buffer: Vec<f32>,
    write_index: usize,
    mask: usize,
}

impl IntegerDelayLine {
    fn new(max_delay_samples: u32) -> Option<Self> {
        let requested = (max_delay_samples as usize).max(1);
        let size = requested.checked_next_power_of_two()?;
        // Bound FFI allocation to 64 MiB per line. Current engine users need
        // at most 65,536 samples; invalid oversized requests fail closed.
        if size > (1 << 24) {
            return None;
        }
        Some(Self {
            buffer: vec![0.0; size],
            write_index: 0,
            mask: size - 1,
        })
    }

    fn push(&mut self, sample: f32) {
        self.buffer[self.write_index] = if sample.is_finite() { sample } else { 0.0 };
        self.write_index = (self.write_index + 1) & self.mask;
    }

    fn read(&self, delay_samples: u32) -> f32 {
        let delay = (delay_samples as usize).min(self.mask);
        let index = self.write_index.wrapping_sub(1 + delay) & self.mask;
        let output = self.buffer[index];
        if output.is_finite() {
            output
        } else {
            0.0
        }
    }

    fn process(&mut self, sample: f32, delay_samples: u32) -> f32 {
        self.push(sample);
        self.read(delay_samples)
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_index = 0;
    }
}

#[no_mangle]
pub extern "C" fn hirari_integer_delay_create(max_delay_samples: u32) -> *mut c_void {
    let Some(state) = IntegerDelayLine::new(max_delay_samples) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(state)).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_integer_delay_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: the handle is allocated by `hirari_integer_delay_create`.
        unsafe { drop(Box::from_raw(state.cast::<IntegerDelayLine>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_integer_delay_process(
    state: *mut c_void,
    sample: f32,
    delay_samples: u32,
) -> f32 {
    let Some(state) = (unsafe { state.cast::<IntegerDelayLine>().as_mut() }) else {
        return 0.0;
    };
    state.process(sample, delay_samples)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_integer_delay_push(state: *mut c_void, sample: f32) {
    if let Some(state) = unsafe { state.cast::<IntegerDelayLine>().as_mut() } {
        state.push(sample);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_integer_delay_read(
    state: *const c_void,
    delay_samples: u32,
) -> f32 {
    unsafe { state.cast::<IntegerDelayLine>().as_ref() }
        .map_or(0.0, |state| state.read(delay_samples))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_integer_delay_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<IntegerDelayLine>().as_mut() } {
        state.reset();
    }
}

impl DelayLineEngine {
    pub fn new(max_delay_samples: u32) -> Self {
        let mut mask = 1;
        let requested = (max_delay_samples as usize).max(1);
        while mask < requested && mask < (1usize << (usize::BITS - 2)) {
            mask <<= 1;
        }
        let buffer = vec![0.0; mask];
        mask -= 1;

        Self {
            buffer,
            write_idx: 0,
            mask,
        }
    }

    pub fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_idx = 0;
    }

    /**
     * @brief PROCESS: Writes a sample and reads delayed output using fractional linear interpolation.
     * INDUSTRIAL: Prevents aliasing, zipper noise, and digitizing clicks during real-time LFO modulation.
     */
    pub fn process(&mut self, sample: f32, delay_samples: f32) -> f32 {
        let sample = if sample.is_finite() { sample } else { 0.0 };
        if self.buffer.is_empty() {
            return 0.0;
        }
        self.write_idx %= self.buffer.len();
        if delay_samples.is_nan() || delay_samples <= 0.0 {
            self.buffer[self.write_idx] = sample;
            self.write_idx = (self.write_idx + 1) & self.mask;
            return sample;
        }

        self.buffer[self.write_idx] = sample;

        // Linear interpolation calculations
        let max_delay = self.buffer.len().saturating_sub(1) as f32;
        let safe_delay = if delay_samples.is_finite() {
            delay_samples.min(max_delay)
        } else {
            max_delay
        };
        let delay_int = safe_delay.floor() as usize;
        let delay_frac = safe_delay - delay_int as f32;

        let r_idx0 = (self.write_idx.wrapping_sub(delay_int)) & self.mask;
        let r_idx1 = (self.write_idx.wrapping_sub(delay_int + 1)) & self.mask;

        let s0 = self.buffer[r_idx0];
        let s1 = self.buffer[r_idx1];

        // Advance write pointer
        self.write_idx = (self.write_idx + 1) & self.mask;

        // Interpolated result
        let output = s0 + (s1 - s0) * delay_frac;
        if output.is_finite() {
            output
        } else {
            0.0
        }
    }

    pub fn audit_delay_line(&self) -> bool {
        !self.buffer.is_empty()
            && self.buffer.len().is_power_of_two()
            && self.mask + 1 == self.buffer.len()
            && self.write_idx < self.buffer.len()
            && self.buffer.iter().all(|sample| sample.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::DelayLineEngine;

    #[test]
    fn rejects_corrupt_public_state_without_panicking() {
        let mut delay = DelayLineEngine::new(8);
        delay.write_idx = usize::MAX;
        delay.buffer[0] = f32::NAN;
        let output = delay.process(f32::NAN, 2.0);
        assert!(output.is_finite());
        assert!(!delay.audit_delay_line());
    }
}

#[cfg(test)]
mod integer_delay_tests {
    use super::IntegerDelayLine;

    #[test]
    fn integer_delay_matches_push_read_and_process_contracts() {
        let mut delay = IntegerDelayLine::new(4).unwrap();
        delay.push(1.0);
        assert_eq!(delay.read(0), 1.0);
        delay.push(2.0);
        assert_eq!(delay.read(1), 1.0);
        assert_eq!(delay.read(0), 2.0);
        assert_eq!(delay.process(3.0, 2), 1.0);
        assert_eq!(delay.process(4.0, 0), 4.0);
    }

    #[test]
    fn integer_delay_sanitizes_samples_clamps_delay_and_resets() {
        let mut delay = IntegerDelayLine::new(3).unwrap();
        assert_eq!(delay.process(f32::NAN, 0), 0.0);
        delay.process(1.0, 0);
        delay.process(2.0, 0);
        delay.process(3.0, 0);
        assert_eq!(delay.process(4.0, u32::MAX), 1.0);
        delay.reset();
        assert_eq!(delay.process(5.0, 3), 0.0);
    }

    #[test]
    fn integer_delay_rejects_unreasonable_storage_requests() {
        assert!(IntegerDelayLine::new(u32::MAX).is_none());
    }
}
