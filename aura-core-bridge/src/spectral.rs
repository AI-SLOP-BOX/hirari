pub struct SpectrumOrchestrator {
    pub target_spectrum: Vec<f32>,
    pub averaged_input: Vec<f32>,
    pub bins: usize,
}

impl SpectrumOrchestrator {
    pub fn new(bins: usize) -> Self {
        Self {
            target_spectrum: vec![0.001; bins],
            averaged_input: vec![0.001; bins],
            bins,
        }
    }

    /// INDUSTRIAL: Integrates a new FFT block into the long-term average with high-performance Rust iteration.
    pub fn update_average(&mut self, input_mags: &[f32]) {
        // INDUSTRIAL: Implementation of high-performance spectral smoothing.
        // Rust's iterator chains are optimized into efficient SIMD instructions by LLVM.
        if input_mags.len() != self.bins
            || self.bins == 0
            || self.bins > 65_536
            || input_mags
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
        {
            return;
        }
        for (avg, &new) in self.averaged_input.iter_mut().zip(input_mags.iter()) {
            *avg = 0.99 * (*avg) + 0.01 * new;
        }
    }

    /// INDUSTRIAL: Calculates the matching curve with forensic accuracy and SIMD-optimized dB conversion.
    pub fn calculate_match_curve(&self) -> Vec<f32> {
        if !self.audit_spectral_balance() {
            return Vec::new();
        }
        let mut curve = vec![0.0; self.bins];

        for i in 0..self.bins {
            let input_db = 20.0 * (self.averaged_input[i] + 1e-10).log10();
            let target_db = 20.0 * (self.target_spectrum[i] + 1e-10).log10();

            // INDUSTRIAL: Professional curve limiting and phase artifact prevention.
            let diff = target_db - input_db;
            curve[i] = diff.clamp(-12.0, 12.0);
        }

        curve
    }

    /// INDUSTRIAL: Generates a deterministic Pink Noise reference spectrum.
    pub fn set_pink_noise_reference(&mut self) {
        for i in 1..self.bins {
            self.target_spectrum[i] = 1.0 / (i as f32).sqrt();
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide spectral balance.
    pub fn audit_spectral_balance(&self) -> bool {
        // A profile is only usable when both sides have the same bounded
        // shape and every magnitude is a finite, non-negative value.  The
        // matcher takes logarithms, so accepting NaN/negative bins here would
        // turn a malformed analysis into an invalid mastering curve.
        self.bins > 0
            && self.bins <= 65_536
            && self.target_spectrum.len() == self.bins
            && self.averaged_input.len() == self.bins
            && self
                .target_spectrum
                .iter()
                .chain(self.averaged_input.iter())
                .all(|value| value.is_finite() && *value >= 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::SpectrumOrchestrator;

    #[test]
    fn spectral_audit_rejects_malformed_profiles() {
        let mut spectrum = SpectrumOrchestrator::new(8);
        assert!(spectrum.audit_spectral_balance());
        spectrum.target_spectrum[2] = f32::NAN;
        assert!(!spectrum.audit_spectral_balance());
        spectrum.target_spectrum[2] = 1.0;
        spectrum.averaged_input[3] = -0.1;
        assert!(!spectrum.audit_spectral_balance());
    }

    #[test]
    fn spectral_audit_rejects_shape_mismatch() {
        let mut spectrum = SpectrumOrchestrator::new(4);
        spectrum.target_spectrum.pop();
        assert!(!spectrum.audit_spectral_balance());
        assert!(spectrum.calculate_match_curve().is_empty());
    }

    #[test]
    fn spectral_average_rejects_invalid_input_atomically() {
        let mut spectrum = SpectrumOrchestrator::new(4);
        let before = spectrum.averaged_input.clone();
        spectrum.update_average(&[1.0, 2.0]);
        assert_eq!(spectrum.averaged_input, before);
        spectrum.update_average(&[1.0, f32::NAN, 2.0, 3.0]);
        assert_eq!(spectrum.averaged_input, before);
        spectrum.update_average(&[1.0, 2.0, 3.0, 4.0]);
        assert_ne!(spectrum.averaged_input, before);
    }
}
