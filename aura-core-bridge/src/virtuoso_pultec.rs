use crate::state_variable_filter::{StateVariableFilterEngine, SvfType};

pub struct VirtuosoPultecEngine {
    pub sample_rate: f64,
    pub low_shelf: StateVariableFilterEngine,
    pub high_shelf: StateVariableFilterEngine,
    pub low_freq: f32,
    pub low_boost: f32,
    pub low_atten: f32,
    pub high_freq: f32,
    pub high_boost: f32,
}

impl VirtuosoPultecEngine {
    pub fn new(sr: f64) -> Self {
        let sample_rate = if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) { sr } else { 48_000.0 };
        let mut low_shelf = StateVariableFilterEngine::new(sample_rate);
        let mut high_shelf = StateVariableFilterEngine::new(sample_rate);
        low_shelf.filter_type = SvfType::LowShelf;
        high_shelf.filter_type = SvfType::HighShelf;

        Self {
            sample_rate,
            low_shelf,
            high_shelf,
            low_freq: 60.0,
            low_boost: 2.0,
            low_atten: 1.0,
            high_freq: 12000.0,
            high_boost: 3.0,
        }
    }

    pub fn reset(&mut self) {
        self.low_shelf.reset();
        self.high_shelf.reset();
    }

    /// INDUSTRIAL: Legendary Passive Program Equalizer (EQP-1A Emulation).
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if l.is_empty() || r.is_empty() || !self.audit_virtuoso_pultec() {
            return;
        }
        // 1. LOW END (Boost + Atten)
        self.low_shelf
            .set_params(self.low_freq, self.low_boost - self.low_atten, 0.707);
        self.low_shelf.process(l, r);

        // 2. HIGH END (Smooth Air)
        self.high_shelf
            .set_params(self.high_freq, self.high_boost, 0.5);
        self.high_shelf.process(l, r);

        // 3. TUBE WARMTH
        let len = l.len().min(r.len());
        for s in 0..len {
            let left = if l[s].is_finite() { l[s] } else { 0.0 };
            let right = if r[s].is_finite() { r[s] } else { 0.0 };
            l[s] = (left * 1.05).tanh() * 0.95;
            r[s] = (right * 1.05).tanh() * 0.95;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Virtuoso Pultec state.
    pub fn audit_virtuoso_pultec(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.low_freq.is_finite()
            && (20.0..self.sample_rate as f32 * 0.49).contains(&self.low_freq)
            && self.high_freq.is_finite()
            && (20.0..self.sample_rate as f32 * 0.49).contains(&self.high_freq)
            && self.low_boost.is_finite()
            && (0.0..=12.0).contains(&self.low_boost)
            && self.low_atten.is_finite()
            && (0.0..=12.0).contains(&self.low_atten)
            && self.high_boost.is_finite()
            && (0.0..=12.0).contains(&self.high_boost)
            && self.low_shelf.audit_state_variable_filter()
            && self.high_shelf.audit_state_variable_filter()
    }
}

#[cfg(test)]
mod tests {
    use super::VirtuosoPultecEngine;

    #[test]
    fn virtuoso_pultec_is_safe_for_mismatched_and_nonfinite_audio() {
        let mut eq = VirtuosoPultecEngine::new(48_000.0);
        let mut left = vec![f32::NAN, 0.5, 2.0];
        let mut right = vec![0.25, 0.25];
        eq.process(&mut left, &mut right);
        assert!(left[..2].iter().chain(right.iter()).all(|v| v.is_finite() && v.abs() <= 1.0));
        assert!(eq.audit_virtuoso_pultec());
    }
}
