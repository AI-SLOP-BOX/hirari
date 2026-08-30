pub struct DelayLineEngine {
    pub buffer: Vec<f32>,
    pub write_idx: usize,
    pub mask: usize,
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
