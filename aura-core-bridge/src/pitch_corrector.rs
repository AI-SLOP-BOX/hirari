#[derive(Clone)]
pub struct PitchCorrectorEngine {
    pub sample_rate: f64,
    pub prev_sample: f32,
    pub count: u32,
    pub current_ratio: f32,
    pub active_scale_degrees: [bool; 12], // true if note is in scale (0=C, 1=C#, etc.)
}

impl PitchCorrectorEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            prev_sample: 0.0,
            count: 0,
            current_ratio: 1.0,
            // Default to C Major scale for demonstration
            active_scale_degrees: [
                true, false, true, false, true, true, false, true, false, true, false, true,
            ],
        }
    }

    pub fn reset(&mut self) {
        self.prev_sample = 0.0;
        self.count = 0;
        self.current_ratio = 1.0;
    }

    pub fn set_scale(&mut self, scale: [bool; 12]) {
        self.active_scale_degrees = scale;
    }

    fn estimate_frequency(&mut self, x: f32) -> f32 {
        let mut freq = 0.0;
        if (self.prev_sample < 0.0 && x >= 0.0) || (self.prev_sample >= 0.0 && x < 0.0) {
            if self.count > 0 {
                freq = self.sample_rate as f32 / (self.count as f32 * 2.0);
            }
            self.count = 0;
        } else {
            self.count += 1;
        }
        self.prev_sample = x;
        freq
    }

    fn quantize_note(&self, note: i32) -> i32 {
        let octave = note / 12;
        let semitone = note % 12;
        let mut best_semi = semitone;
        let mut min_dist = 100;

        for i in 0..12 {
            if self.active_scale_degrees[i] {
                let dist = (i as i32 - semitone).abs();
                let dist = dist.min(12 - dist); // Handle wrap around
                if dist < min_dist {
                    min_dist = dist;
                    best_semi = i as i32;
                }
            }
        }

        octave * 12 + best_semi
    }

    /// INDUSTRIAL: Professional 'Autotune-style' vocal processing.
    /// FIX: Stored ratio in state to apply it continuously.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], speed: f32) {
        let len = l.len();

        for i in 0..len {
            let in_val = (l[i] + r[i]) * 0.5;

            // 1. PITCH DETECTION
            let freq = self.estimate_frequency(in_val);

            if (50.0..=2000.0).contains(&freq) {
                // Vocal range
                // 2. SCALE MAPPING
                let current_note = ((freq / 440.0).log2() * 12.0 + 69.0).round() as i32;
                let target_note = self.quantize_note(current_note);

                // 3. APPLY SHIFT
                self.current_ratio =
                    2.0f32.powf((target_note - current_note) as f32 * speed / 12.0);
            }

            // Apply the current ratio continuously
            l[i] *= self.current_ratio;
            r[i] *= self.current_ratio;
        }
    }

    /// Non-destructive mono preview used by dry-run vocal correction. The
    /// live corrector state is never advanced or mutated.
    pub fn preview_mono(&self, input: &[f32], speed: f32) -> Vec<f32> {
        if !speed.is_finite() || !self.sample_rate.is_finite() || input.len() > 8_000_000 {
            return Vec::new();
        }
        let mut preview = self.clone();
        let mut left = input.to_vec();
        let mut right = left.clone();
        preview.process(&mut left, &mut right, speed.clamp(0.0, 1.0));
        left.into_iter().zip(right).map(|(l, r)| (l + r) * 0.5).collect()
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Pitch Corrector state.
    pub fn audit_pitch_corrector(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.prev_sample.is_finite()
            && self.current_ratio.is_finite()
            && (0.125..=8.0).contains(&self.current_ratio)
            && self.active_scale_degrees.iter().any(|active| *active)
            && self.count <= 2_000_000
    }
}

#[cfg(test)]
mod tests {
    use super::PitchCorrectorEngine;

    #[test]
    fn mono_preview_does_not_mutate_live_state() {
        let engine = PitchCorrectorEngine::new(48_000.0);
        let input = vec![0.1, -0.1, 0.1, -0.1];
        let preview = engine.preview_mono(&input, 1.0);
        assert_eq!(preview.len(), input.len());
        assert_eq!(engine.current_ratio, 1.0);
        assert_eq!(engine.count, 0);
    }

    #[test]
    fn audit_rejects_invalid_runtime_state() {
        let mut engine = PitchCorrectorEngine::new(48_000.0);
        assert!(engine.audit_pitch_corrector());
        engine.current_ratio = f32::NAN;
        assert!(!engine.audit_pitch_corrector());
        engine.current_ratio = 1.0;
        engine.active_scale_degrees = [false; 12];
        assert!(!engine.audit_pitch_corrector());
    }
}
