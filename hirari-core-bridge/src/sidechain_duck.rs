pub struct SidechainDuckEngine {
    pub sample_rate: f64,
    pub depth: f32,
    pub current_gain: f32,
    pub lfo_phase: f64,
}

#[cfg(test)]
mod tests {
    use super::SidechainDuckEngine;

    #[test]
    fn validated_depth_update_rejects_non_finite_values() {
        let mut duck = SidechainDuckEngine::new(48_000.0);
        assert!(duck.try_set_depth(0.5));
        assert!(!duck.try_set_depth(f32::NAN));
        assert_eq!(duck.depth, 0.5);
        assert!(duck.audit_sidechain_duck());
    }
}

impl SidechainDuckEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            depth: 0.8,
            current_gain: 1.0,
            lfo_phase: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.current_gain = 1.0;
        self.lfo_phase = 0.0;
    }

    pub fn set_depth(&mut self, depth: f32) {
        if depth.is_finite() {
            self.depth = depth.clamp(0.0, 1.0);
        }
    }

    pub fn try_set_depth(&mut self, depth: f32) -> bool {
        if !depth.is_finite() || !(0.0..=1.0).contains(&depth) {
            return false;
        }
        self.depth = depth;
        true
    }

    /// INDUSTRIAL: Dynamic Pumping effect for professional Electronic/Trap music.
    pub fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        sidechain: Option<(&[f32], &[f32])>,
        bpm: f64,
    ) {
        let len = l.len().min(r.len());
        if !self.sample_rate.is_finite()
            || self.sample_rate <= 0.0
            || !bpm.is_finite()
            || bpm <= 0.0
        {
            return;
        }
        let samples_per_beat = (60.0 / bpm) * self.sample_rate;

        let exp_scale = -2.0 * std::f64::consts::PI * (1.0 / (self.sample_rate * 0.010)); // 10ms time constant
        let smoothing = (1.0 - exp_scale.exp()) as f32;

        for s in 0..len {
            let reduction = if let Some((sc_l, sc_r)) = sidechain {
                let sc_left = sc_l.get(s).copied().unwrap_or(0.0);
                let sc_right = sc_r.get(s).copied().unwrap_or(0.0);
                let peak = (if sc_left.is_finite() {
                    sc_left.abs()
                } else {
                    0.0
                })
                .max(if sc_right.is_finite() {
                    sc_right.abs()
                } else {
                    0.0
                });
                (peak * 2.0).clamp(0.0, 1.0)
            } else {
                // LFO mode
                self.lfo_phase += 1.0 / samples_per_beat;
                if self.lfo_phase >= 1.0 {
                    self.lfo_phase -= 1.0;
                }

                let p = (1.0 - self.lfo_phase) as f32;
                p * p // Quadratic pump
            };

            let gain = 1.0 - (reduction * self.depth);
            self.current_gain += (gain - self.current_gain) * smoothing;

            l[s] =
                ((if l[s].is_finite() { l[s] } else { 0.0 }) * self.current_gain).clamp(-4.0, 4.0);
            r[s] =
                ((if r[s].is_finite() { r[s] } else { 0.0 }) * self.current_gain).clamp(-4.0, 4.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Sidechain Duck state.
    pub fn audit_sidechain_duck(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 100.0
            && self.depth.is_finite()
            && (0.0..=1.0).contains(&self.depth)
            && self.current_gain.is_finite()
            && (0.0..=1.0).contains(&self.current_gain)
            && self.lfo_phase.is_finite()
    }
}
