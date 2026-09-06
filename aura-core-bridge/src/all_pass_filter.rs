pub struct AllPassFilterEngine {
    pub delay_buffer: Vec<f32>,
    pub feedback: f32,
    pub idx: usize,
    pub mask: usize,
}

impl AllPassFilterEngine {
    pub fn new(delay_samples: usize, feedback: f32) -> Self {
        let size = delay_samples.saturating_add(1).next_power_of_two().max(1);

        Self {
            delay_buffer: vec![0.0; size],
            feedback: if feedback.is_finite() {
                feedback.clamp(-1.0, 1.0)
            } else {
                0.0
            },
            idx: 0,
            mask: size - 1,
        }
    }

    pub fn reset(&mut self) {
        self.delay_buffer.fill(0.0);
        self.idx = 0;
    }

    /// INDUSTRIAL: Phase-shifting without amplitude change for diffusion.
    pub fn process(&mut self, in_val: f32) -> f32 {
        let buffer_out = self.delay_buffer[self.idx];
        let out = -self.feedback * in_val + buffer_out;
        self.delay_buffer[self.idx] = in_val + self.feedback * buffer_out;

        self.idx = (self.idx + 1) & self.mask;
        out
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide All Pass Filter state.
    pub fn audit_all_pass_filter(&self) -> bool {
        !self.delay_buffer.is_empty()
            && self.delay_buffer.len().is_power_of_two()
            && self.mask == self.delay_buffer.len() - 1
            && self.idx < self.delay_buffer.len()
            && self.feedback.is_finite()
            && self.feedback.abs() <= 1.0
            && self.delay_buffer.iter().all(|sample| sample.is_finite())
    }
}
