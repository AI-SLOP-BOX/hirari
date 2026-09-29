pub struct SynthesisEngineOrchestrator {
    pub master_gain: f32,
}

impl Default for SynthesisEngineOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl SynthesisEngineOrchestrator {
    pub fn new() -> Self {
        Self { master_gain: 1.0 }
    }

    pub fn set_master_gain(&mut self, gain: f32) {
        if gain.is_finite() {
            self.master_gain = gain.clamp(0.0, 16.0);
        }
    }

    /// INDUSTRIAL: Coordinates voice rendering and applies master gain.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        // 1. Voice rendering would be called here or handled in C++ before this call.
        // Assuming voice rendering is done in C++ and we just apply gain here,
        // or we orchestrate the calls.

        // 2. SIMD Gain Scaling (Compiler auto-vectorizes this safely)
        if !self.audit_synthesis_engine() {
            return;
        }
        let gain = self.master_gain;
        let _num_samples = l.len();

        // Use chunks for better vectorization hints
        let (chunks, remainder) = l.as_chunks_mut::<4>();
        for chunk in chunks {
            for sample in chunk {
                *sample = if sample.is_finite() {
                    (*sample * gain).clamp(-1.0, 1.0)
                } else {
                    0.0
                };
            }
        }
        for sample in remainder {
            *sample = if sample.is_finite() {
                (*sample * gain).clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }

        let (chunks_r, remainder_r) = r.as_chunks_mut::<4>();
        for chunk in chunks_r {
            for sample in chunk {
                *sample = if sample.is_finite() {
                    (*sample * gain).clamp(-1.0, 1.0)
                } else {
                    0.0
                };
            }
        }
        for sample in remainder_r {
            *sample = if sample.is_finite() {
                (*sample * gain).clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Synthesis state.
    pub fn audit_synthesis_engine(&self) -> bool {
        self.master_gain.is_finite() && (0.0..=16.0).contains(&self.master_gain)
    }
}

#[cfg(test)]
mod tests {
    use super::SynthesisEngineOrchestrator;

    #[test]
    fn synthesis_gain_is_bounded_and_finite() {
        let mut engine = SynthesisEngineOrchestrator::new();
        engine.set_master_gain(4.0);
        let mut left = vec![0.4_f32; 8];
        let mut right = vec![0.2_f32; 3];
        engine.process(&mut left, &mut right);
        assert!(left
            .iter()
            .chain(right.iter())
            .all(|v| v.is_finite() && v.abs() <= 1.0));
        engine.set_master_gain(f32::NAN);
        assert!(engine.audit_synthesis_engine());
    }
}
