pub struct StereoImagerEngine {
    pub sample_rate: f64,
    pub width: f32,
    pub target_side_gain: f32,
    pub current_side_gain: f32,
}

impl StereoImagerEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            width: 1.0,
            target_side_gain: 1.0,
            current_side_gain: 1.0,
        }
    }

    pub fn reset(&mut self) {
        self.current_side_gain = self.target_side_gain;
    }

    pub fn set_width(&mut self, width: f32) {
        self.width = width.clamp(0.0, 4.0);
        self.target_side_gain = self.width;
    }

    /// INDUSTRIAL: Advanced Mid-Side spatial processor.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len().min(r.len());
        if !self.sample_rate.is_finite() || self.sample_rate <= 0.0 {
            return;
        }

        for i in 0..len {
            let in_l = if l[i].is_finite() { l[i] } else { 0.0 };
            let in_r = if r[i].is_finite() { r[i] } else { 0.0 };

            // 1. MID-SIDE ENCODING
            let mid = (in_l + in_r) * 0.5;
            let mut side = (in_l - in_r) * 0.5;

            // 2. SPATIAL SCULPTING (Width Control)
            // Note: Smooth gain change to prevent clicks
            self.current_side_gain += (self.target_side_gain - self.current_side_gain) * 0.01;
            side *= self.current_side_gain;

            // 3. MID-SIDE DECODING (Back to L/R)
            // Compensation: Boost Mid slightly if Side is very wide to keep perceived power
            let comp = 1.0 / 1.0f32.max(self.current_side_gain * 0.5);

            l[i] = ((mid + side) * comp).clamp(-4.0, 4.0);
            r[i] = ((mid - side) * comp).clamp(-4.0, 4.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Stereo Imager state.
    pub fn audit_stereo_imager(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 100.0
            && self.width.is_finite()
            && (0.0..=4.0).contains(&self.width)
            && self.target_side_gain.is_finite()
            && self.current_side_gain.is_finite()
    }
}
