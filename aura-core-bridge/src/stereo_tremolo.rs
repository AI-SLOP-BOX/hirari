pub struct StereoTremoloEngine {
    pub sample_rate: f64,
    pub lfo_phase: f64,
    pub depth: f32,
    pub note_value: f32,
    pub stereo_width: f32,
}

impl StereoTremoloEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: if sr.is_finite() && (100.0..=384_000.0).contains(&sr) {
                sr
            } else {
                44_100.0
            },
            lfo_phase: 0.0,
            depth: 0.0,
            note_value: 0.25,
            stereo_width: 0.5,
        }
    }

    pub fn reset(&mut self) {
        self.lfo_phase = 0.0;
    }

    pub fn set_depth(&mut self, d: f32) {
        self.depth = if d.is_finite() {
            d.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }

    pub fn set_note_value(&mut self, v: f32) {
        self.note_value = if v.is_finite() {
            v.clamp(1.0e-4, 64.0)
        } else {
            0.25
        };
    }

    pub fn set_stereo_width(&mut self, w: f32) {
        self.stereo_width = if w.is_finite() {
            w.clamp(0.0, 1.0)
        } else {
            0.5
        };
    }

    /// INDUSTRIAL: Rhythmic volume and pan modulation.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], bpm: f64, current_sample_rate: f64) {
        let len = l.len().min(r.len());
        if len == 0
            || !bpm.is_finite()
            || !(1.0e-3..=1.0e4).contains(&bpm)
            || !current_sample_rate.is_finite()
            || !(100.0..=384_000.0).contains(&current_sample_rate)
            || !self.lfo_phase.is_finite()
            || !self.depth.is_finite()
            || !(0.0..=1.0).contains(&self.depth)
            || !self.note_value.is_finite()
            || !(1.0e-4..=64.0).contains(&self.note_value)
            || !self.stereo_width.is_finite()
            || !(0.0..=1.0).contains(&self.stereo_width)
        {
            return;
        }

        let denominator = 60.0 * current_sample_rate * self.note_value as f64;
        if !denominator.is_finite() || denominator <= 0.0 {
            return;
        }
        let lfo_inc = bpm / denominator;
        if !lfo_inc.is_finite() || lfo_inc < 0.0 {
            return;
        }

        self.lfo_phase = self.lfo_phase.rem_euclid(1.0);

        for s in 0..len {
            self.lfo_phase = (self.lfo_phase + lfo_inc).rem_euclid(1.0);

            // Sine LFO for smooth pulsing
            let lfo_l = 0.5 + 0.5 * (2.0 * std::f64::consts::PI * self.lfo_phase).sin();
            let lfo_r = 0.5
                + 0.5
                    * (2.0 * std::f64::consts::PI * self.lfo_phase
                        + std::f64::consts::PI * self.stereo_width as f64)
                        .sin();

            let mod_l = 1.0 - (self.depth * lfo_l as f32);
            let mod_r = 1.0 - (self.depth * lfo_r as f32);

            let in_l = if l[s].is_finite() { l[s] } else { 0.0 };
            let in_r = if r[s].is_finite() { r[s] } else { 0.0 };
            l[s] = (in_l * mod_l).clamp(-4.0, 4.0);
            r[s] = (in_r * mod_r).clamp(-4.0, 4.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Stereo Tremolo state.
    pub fn audit_stereo_tremolo(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 100.0
            && self.sample_rate <= 384_000.0
            && self.lfo_phase.is_finite()
            && self.depth.is_finite()
            && (0.0..=1.0).contains(&self.depth)
            && self.note_value.is_finite()
            && (1.0e-4..=64.0).contains(&self.note_value)
            && self.stereo_width.is_finite()
            && (0.0..=1.0).contains(&self.stereo_width)
    }
}

#[cfg(test)]
mod tests {
    use super::StereoTremoloEngine;

    #[test]
    fn setters_reject_non_finite_and_bound_parameters() {
        let mut engine = StereoTremoloEngine::new(f64::NAN);
        assert!(engine.audit_stereo_tremolo());

        engine.set_depth(f32::NAN);
        engine.set_note_value(f32::INFINITY);
        engine.set_stereo_width(f32::NEG_INFINITY);

        assert_eq!(engine.depth, 0.0);
        assert_eq!(engine.note_value, 0.25);
        assert_eq!(engine.stereo_width, 0.5);
        assert!(engine.audit_stereo_tremolo());
    }

    #[test]
    fn process_sanitizes_audio_and_keeps_phase_finite() {
        let mut engine = StereoTremoloEngine::new(44_100.0);
        engine.set_depth(1.0);
        engine.lfo_phase = f64::MAX;
        let mut left = [f32::NAN, 1.0e30];
        let mut right = [f32::INFINITY, -1.0e30];

        engine.process(&mut left, &mut right, 120.0, 44_100.0);

        assert!(engine.lfo_phase.is_finite());
        assert!(left
            .iter()
            .chain(right.iter())
            .all(|sample| sample.is_finite()));
        assert!(left
            .iter()
            .chain(right.iter())
            .all(|sample| sample.abs() <= 4.0));
    }

    #[test]
    fn invalid_timing_does_not_modify_audio() {
        let mut engine = StereoTremoloEngine::new(44_100.0);
        let mut left = [0.25, -0.5];
        let mut right = [0.5, -0.25];
        let before_left = left;
        let before_right = right;

        engine.process(&mut left, &mut right, f64::NAN, 44_100.0);

        assert_eq!(left, before_left);
        assert_eq!(right, before_right);
    }
}
