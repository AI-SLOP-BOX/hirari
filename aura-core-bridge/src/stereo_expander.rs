pub struct StereoExpanderEngine {
    pub width: f32,
    pub mid_gain: f32,
}

impl Default for StereoExpanderEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl StereoExpanderEngine {
    pub fn new() -> Self {
        Self {
            width: 1.0,
            mid_gain: 1.0,
        }
    }

    pub fn set_params(&mut self, width: f32, mid_gain: f32) {
        if width.is_finite() && mid_gain.is_finite() {
            self.width = width.clamp(0.0, 2.0);
            self.mid_gain = mid_gain.clamp(0.0, 2.0);
        }
    }

    /// INDUSTRIAL: M/S Matrixing and Width expansion.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if !self.audit_stereo_expander() {
            return;
        }
        let len = l.len().min(r.len());

        for s in 0..len {
            let in_l = l[s];
            let in_r = r[s];

            // 1. L/R to M/S Matrix
            let mid = (in_l + in_r) * 0.5;
            let side = (in_l - in_r) * 0.5;

            // 2. Apply Width and Gains
            let mid_processed = mid * self.mid_gain;
            let side_processed = side * self.width;

            // 3. M/S to L/R Matrix (Inverse)
            l[s] = (mid_processed + side_processed).clamp(-4.0, 4.0);
            r[s] = (mid_processed - side_processed).clamp(-4.0, 4.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Stereo Expander state.
    pub fn audit_stereo_expander(&self) -> bool {
        self.width.is_finite()
            && (0.0..=2.0).contains(&self.width)
            && self.mid_gain.is_finite()
            && (0.0..=2.0).contains(&self.mid_gain)
    }
}
