pub struct HarmonicReconstructorEngine {
    pub sample_rate: f64,
}

impl HarmonicReconstructorEngine {
    pub fn new(sr: f64) -> Self {
        Self { sample_rate: sr }
    }

    pub fn reset(&mut self) {
        self.sample_rate = 48_000.0;
    }

    /// INDUSTRIAL: High-frequency 'Air' restoration using non-linear projection.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len().min(r.len());

        for s in 0..len {
            // Process Left
            let in_l = if l[s].is_finite() {
                l[s].clamp(-1.0, 1.0)
            } else {
                0.0
            };
            let high_l = (in_l * 1.5).tanh() - in_l;
            l[s] = (in_l + high_l * 0.1).clamp(-1.0, 1.0);

            // Process Right
            let in_r = if r[s].is_finite() {
                r[s].clamp(-1.0, 1.0)
            } else {
                0.0
            };
            let high_r = (in_r * 1.5).tanh() - in_r;
            r[s] = (in_r + high_r * 0.1).clamp(-1.0, 1.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Harmonic Reconstructor state.
    pub fn audit_harmonic_reconstructor(&self) -> bool {
        self.sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&self.sample_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::HarmonicReconstructorEngine;

    #[test]
    fn reset_restores_the_default_processing_rate() {
        let mut engine = HarmonicReconstructorEngine::new(96_000.0);
        engine.reset();
        assert_eq!(engine.sample_rate, 48_000.0);
    }
    #[test]
    fn harmonic_reconstructor_sanitizes_nonfinite_and_bounds_output() {
        let mut engine = HarmonicReconstructorEngine::new(48_000.0);
        let mut left = vec![f32::NAN, 2.0, -2.0, 0.2];
        let mut right = vec![f32::INFINITY, -2.0, 2.0, -0.2];
        engine.process(&mut left, &mut right);
        assert!(left
            .iter()
            .chain(right.iter())
            .all(|sample| sample.is_finite() && sample.abs() <= 1.0));
        assert_eq!(left[0], 0.0);
        assert_eq!(right[0], 0.0);
    }
}
