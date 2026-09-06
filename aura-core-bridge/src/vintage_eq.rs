pub struct BiquadFilter {
    pub x1: f32,
    pub x2: f32,
    pub y1: f32,
    pub y2: f32,
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Default for BiquadFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl BiquadFilter {
    pub fn new() -> Self {
        Self {
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    pub fn process(&mut self, in_val: f32) -> f32 {
        let out = self.b0 * in_val + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = in_val;
        self.y2 = self.y1;
        self.y1 = out;
        out
    }
}

pub struct VintageEqEngine {
    pub sample_rate: f64,
    pub low_boost: f32,
    pub low_atten: f32,
    pub high_boost: f32,
    pub low_filters: [BiquadFilter; 2],
    pub high_filters: [BiquadFilter; 2],
}

impl VintageEqEngine {
    pub fn new(sr: f64) -> Self {
        let sample_rate = if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) {
            sr
        } else {
            48_000.0
        };
        let mut engine = Self {
            sample_rate,
            low_boost: 2.0,
            low_atten: 1.0,
            high_boost: 3.0,
            low_filters: [BiquadFilter::new(), BiquadFilter::new()],
            high_filters: [BiquadFilter::new(), BiquadFilter::new()],
        };
        engine.update_coefficients();
        engine
    }

    pub fn reset(&mut self) {
        for f in self.low_filters.iter_mut() {
            f.reset();
        }
        for f in self.high_filters.iter_mut() {
            f.reset();
        }
    }

    pub fn set_low_boost(&mut self, b: f32) {
        if b.is_finite() && (0.0..=12.0).contains(&b) {
            self.low_boost = b;
            self.update_coefficients();
        }
    }

    pub fn set_low_atten(&mut self, a: f32) {
        if a.is_finite() && (0.0..=12.0).contains(&a) {
            self.low_atten = a;
            self.update_coefficients();
        }
    }

    pub fn set_high_boost(&mut self, b: f32) {
        if b.is_finite() && (0.0..=12.0).contains(&b) {
            self.high_boost = b;
            self.update_coefficients();
        }
    }

    fn update_coefficients(&mut self) {
        // INDUSTRIAL: Implementation of Pultec-style R-L-C transfer function approximation.
        // For now, setting up basic Biquad coefficients to ensure audio passes correctly.
        // In a full implementation, these would be calculated based on the parameters.
        let sr = if self.sample_rate.is_finite() {
            self.sample_rate.clamp(1000.0, 384000.0) as f32
        } else {
            44100.0
        };
        let shelf = |gain: f32, high: bool| {
            let a = 10.0f32.powf(gain.clamp(-12.0, 12.0) / 40.0);
            let w = 2.0 * std::f32::consts::PI * 1000.0 / sr;
            let c = w.cos();
            let s = w.sin();
            let alpha = s * 0.5;
            let beta = 2.0 * a.sqrt() * alpha.sqrt();
            let (b0, b1, b2, a0, a1, a2) = if high {
                (
                    a * ((a + 1.0) + (a - 1.0) * c + beta),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                    a * ((a + 1.0) + (a - 1.0) * c - beta),
                    (a + 1.0) - (a - 1.0) * c + beta,
                    2.0 * ((a - 1.0) - (a + 1.0) * c),
                    (a + 1.0) - (a - 1.0) * c - beta,
                )
            } else {
                (
                    a * ((a + 1.0) - (a - 1.0) * c + beta),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                    a * ((a + 1.0) - (a - 1.0) * c - beta),
                    (a + 1.0) + (a - 1.0) * c + beta,
                    -2.0 * ((a - 1.0) + (a + 1.0) * c),
                    (a + 1.0) + (a - 1.0) * c - beta,
                )
            };
            (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
        };
        let low = shelf(
            self.low_boost.clamp(0.0, 12.0) - self.low_atten.clamp(0.0, 12.0),
            false,
        );
        let high = shelf(self.high_boost.clamp(0.0, 12.0), true);
        for f in self.low_filters.iter_mut() {
            (f.b0, f.b1, f.b2, f.a1, f.a2) = low;
        }
        for f in self.high_filters.iter_mut() {
            (f.b0, f.b1, f.b2, f.a1, f.a2) = high;
        }
    }

    /// INDUSTRIAL: Applies the unique passive EQ curves.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len().min(r.len());
        if len == 0 || !self.audit_vintage_eq() {
            return;
        }

        for s in 0..len {
            // Left
            let in_l = if l[s].is_finite() { l[s] } else { 0.0 };
            let low_l = self.low_filters[0].process(in_l);
            let high_l = self.high_filters[0].process(low_l);
            l[s] = high_l.clamp(-1.0, 1.0);

            // Right
            let in_r = if r[s].is_finite() { r[s] } else { 0.0 };
            let low_r = self.low_filters[1].process(in_r);
            let high_r = self.high_filters[1].process(low_r);
            r[s] = high_r.clamp(-1.0, 1.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Vintage EQ state.
    pub fn audit_vintage_eq(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.low_boost.is_finite()
            && (0.0..=12.0).contains(&self.low_boost)
            && self.low_atten.is_finite()
            && (0.0..=12.0).contains(&self.low_atten)
            && self.high_boost.is_finite()
            && (0.0..=12.0).contains(&self.high_boost)
            && self
                .low_filters
                .iter()
                .chain(self.high_filters.iter())
                .all(|filter| {
                    [filter.x1, filter.x2, filter.y1, filter.y2, filter.b0, filter.b1, filter.b2, filter.a1, filter.a2]
                        .iter()
                        .all(|value| value.is_finite() && value.abs() <= 16.0)
                        && 1.0 + filter.a1 + filter.a2 > 0.0
                        && 1.0 - filter.a1 + filter.a2 > 0.0
                        && 1.0 - filter.a2 > 0.0
                })
    }
}

#[cfg(test)]
mod tests {
    use super::VintageEqEngine;

    #[test]
    fn vintage_eq_initializes_coefficients_and_sanitizes_audio() {
        let mut eq = VintageEqEngine::new(48_000.0);
        let mut left = vec![f32::NAN, 0.5, 2.0];
        let mut right = vec![0.25, 0.25, 0.25];
        eq.process(&mut left, &mut right);
        assert!(left.iter().chain(right.iter()).all(|v| v.is_finite() && v.abs() <= 1.0));
        assert!(eq.audit_vintage_eq());
    }

    #[test]
    fn vintage_eq_audit_rejects_corrupt_state() {
        let mut eq = VintageEqEngine::new(48_000.0);
        eq.low_boost = f32::NAN;
        assert!(!eq.audit_vintage_eq());
    }

    #[test]
    fn vintage_eq_rejects_invalid_parameter_updates_atomically() {
        let mut eq = VintageEqEngine::new(48_000.0);
        let before = eq.low_boost;
        eq.set_low_boost(f32::NAN);
        eq.set_low_atten(-1.0);
        eq.set_high_boost(99.0);
        assert_eq!(eq.low_boost, before);
        assert_eq!(eq.low_atten, 1.0);
        assert_eq!(eq.high_boost, 3.0);
        assert!(eq.audit_vintage_eq());
    }
}
