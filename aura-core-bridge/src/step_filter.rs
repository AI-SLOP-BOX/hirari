pub struct StepFilterEngine {
    pub sample_rate: f64,
    pub res: f32,
    pub smooth_cutoff: f32,
    pub step_values: [f32; 16],
    pub filter_state: [f32; 2],
}

impl StepFilterEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            res: 0.1,
            smooth_cutoff: 0.5,
            step_values: [0.5; 16],
            filter_state: [0.0; 2],
        }
    }

    pub fn reset(&mut self) {
        self.filter_state = [0.0; 2];
        self.smooth_cutoff = self.step_values[0];
    }

    pub fn set_step_value(&mut self, step: u32, val: f32) {
        if step < 16 {
            self.step_values[step as usize] = val;
        }
    }

    pub fn set_resonance(&mut self, r: f32) {
        self.res = r;
    }

    /// INDUSTRIAL: Modulates filter based on the rhythmic grid.
    pub fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        bpm: f64,
        current_sample_rate: f64,
        playhead: u64,
    ) {
        let len = l.len().min(r.len());
        let samples_per_beat = if bpm.is_finite()
            && bpm > 0.0
            && current_sample_rate.is_finite()
            && current_sample_rate > 0.0
        {
            (60.0 / bpm) * current_sample_rate
        } else {
            1.0
        };
        let samples_per_step = (samples_per_beat * 0.25).max(f64::MIN_POSITIVE); // 16th Note

        // Keep the pole calculation finite even if the engine is constructed with
        // an invalid sample rate.  The fallback is only for invalid input; normal
        // operation uses the configured sample rate unchanged.
        let sample_rate = if self.sample_rate.is_finite() && self.sample_rate > 0.0 {
            self.sample_rate
        } else {
            44100.0
        };
        let nyquist = sample_rate * 0.5;
        let max_cutoff = (nyquist * (1.0 - 1.0e-6)).max(100.0);

        for s in 0..len {
            // 1. Determine Current Step (Sync'd to transport)
            let current_global_pos = playhead.saturating_add(s as u64);
            let step = ((current_global_pos as f64 / samples_per_step) as usize) % 16;

            // 2. Smooth Step Modulation (Inter-step interpolation)
            let target_cutoff = self.step_values[step];
            self.smooth_cutoff = 0.99 * self.smooth_cutoff + 0.01 * target_cutoff;

            // 3. Filter Processing (Simplified Moog-style Ladder)
            let cutoff = if self.smooth_cutoff.is_finite() {
                self.smooth_cutoff as f64 * 8000.0
            } else {
                100.0
            };
            let f = cutoff.clamp(100.0, 20000.0).min(max_cutoff);
            let angle = (std::f64::consts::PI * f / sample_rate)
                .clamp(0.0, std::f64::consts::FRAC_PI_2 - 1.0e-6);
            let g = angle.tan() as f32;
            let requested_k = if self.res.is_finite() {
                (3.0 * self.res).clamp(0.0, 1.0)
            } else {
                0.0
            };
            // Bound the recursive coefficient so a high cutoff cannot turn the
            // one-pole approximation into an explosive resonator.
            let k = (requested_k * g).min(0.999) / g.max(f32::MIN_POSITIVE);

            // Left
            let in_l = if l[s].is_finite() { l[s] } else { 0.0 };
            if !self.filter_state[0].is_finite() {
                self.filter_state[0] = 0.0;
            }
            self.filter_state[0] = (in_l - k * self.filter_state[0]) * g + self.filter_state[0];
            if !self.filter_state[0].is_finite() {
                self.filter_state[0] = 0.0;
            }
            l[s] = self.filter_state[0];

            // Right
            let in_r = if r[s].is_finite() { r[s] } else { 0.0 };
            if !self.filter_state[1].is_finite() {
                self.filter_state[1] = 0.0;
            }
            self.filter_state[1] = (in_r - k * self.filter_state[1]) * g + self.filter_state[1];
            if !self.filter_state[1].is_finite() {
                self.filter_state[1] = 0.0;
            }
            r[s] = self.filter_state[1];
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Step Filter state.
    pub fn audit_step_filter(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 100.0
            && self.res.is_finite()
            && self.smooth_cutoff.is_finite()
            && self.filter_state.iter().all(|value| value.is_finite())
            && self.step_values.iter().all(|value| value.is_finite())
    }
}
