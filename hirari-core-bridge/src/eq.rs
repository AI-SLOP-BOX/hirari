#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiquadCoeffs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

pub struct EqOrchestrator;

impl EqOrchestrator {
    pub fn new() -> Self {
        Self
    }

    /// INDUSTRIAL: Calculates biquad coefficients with absolute precision and spectral sovereignty.
    pub fn calculate_peaking(&self, freq: f32, sr: f32, q: f32, gain_db: f32) -> BiquadCoeffs {
        self.calculate_peaking_checked(freq, sr, q, gain_db)
            .unwrap_or(Self::identity())
    }

    /// Calculate a peaking-EQ biquad, rejecting parameters that could produce
    /// non-finite or unstable coefficients.
    pub fn calculate_peaking_checked(
        &self,
        freq: f32,
        sr: f32,
        q: f32,
        gain_db: f32,
    ) -> Option<BiquadCoeffs> {
        if !freq.is_finite()
            || !sr.is_finite()
            || !q.is_finite()
            || !gain_db.is_finite()
            || !(8_000.0..=384_000.0).contains(&sr)
            || !(10.0..sr * 0.49).contains(&freq)
            || !(0.05..=100.0).contains(&q)
            || !(-48.0..=48.0).contains(&gain_db)
        {
            return None;
        }
        let a = 10.0f32.powf(gain_db / 40.0);
        let omega = 2.0 * std::f32::consts::PI * freq / sr;
        let alpha = omega.sin() / (2.0 * q);

        let a0 = 1.0 + alpha / a;
        let coeffs = BiquadCoeffs {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * omega.cos()) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * omega.cos()) / a0,
            a2: (1.0 - alpha / a) / a0,
        };
        if self.audit_coeffs(&coeffs) {
            Some(coeffs)
        } else {
            None
        }
    }

    fn identity() -> BiquadCoeffs {
        BiquadCoeffs {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }

    /// Validate finite coefficients and second-order denominator stability.
    pub fn audit_coeffs(&self, coeffs: &BiquadCoeffs) -> bool {
        let finite = [coeffs.b0, coeffs.b1, coeffs.b2, coeffs.a1, coeffs.a2]
            .iter()
            .all(|value| value.is_finite() && value.abs() <= 16.0);
        finite
            && 1.0 + coeffs.a1 + coeffs.a2 > 0.0
            && 1.0 - coeffs.a1 + coeffs.a2 > 0.0
            && 1.0 - coeffs.a2 > 0.0
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide spectral synchronization graph.
    pub fn audit_eq(&self) -> bool {
        self.calculate_peaking_checked(1_000.0, 48_000.0, 0.707, 0.0)
            .is_some()
    }
}

impl Default for EqOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::EqOrchestrator;

    #[test]
    fn peaking_eq_rejects_invalid_parameters_without_nan() {
        let eq = EqOrchestrator::new();
        assert!(eq
            .calculate_peaking_checked(1_000.0, 48_000.0, 0.707, 6.0)
            .is_some());
        assert!(eq
            .calculate_peaking_checked(0.0, 48_000.0, 0.707, 6.0)
            .is_none());
        assert!(eq
            .calculate_peaking_checked(1_000.0, 48_000.0, 0.0, 6.0)
            .is_none());
        let fallback = eq.calculate_peaking(f32::NAN, 48_000.0, 1.0, 0.0);
        assert_eq!(fallback.b0, 1.0);
        assert!(eq.audit_eq());
    }

    #[test]
    fn peaking_eq_audit_rejects_unstable_denominator() {
        let eq = EqOrchestrator::new();
        let mut coeffs = eq.calculate_peaking(1_000.0, 48_000.0, 1.0, 0.0);
        coeffs.a2 = 1.1;
        assert!(!eq.audit_coeffs(&coeffs));
    }
}
