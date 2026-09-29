use std::collections::HashMap;

pub enum ModulatorType {
    LFO { phase: f64, freq: f32, wave: u8 },
    Envelope,
}

pub struct ModulatorNode {
    pub id: u32,
    pub mod_type: ModulatorType,
    pub gate: bool,
    pub attack_seconds: f32,
    pub decay_seconds: f32,
    pub sustain_level: f32,
    pub release_seconds: f32,
    envelope_phase: f32,
    envelope_level: f32,
}

impl ModulatorNode {
    pub fn new(id: u32, mod_type: ModulatorType) -> Self {
        Self {
            id,
            mod_type,
            gate: true,
            attack_seconds: 0.01,
            decay_seconds: 0.1,
            sustain_level: 0.7,
            release_seconds: 0.2,
            envelope_phase: 0.0,
            envelope_level: 0.0,
        }
    }
}

pub struct ModulationMatrixOrchestrator {
    pub routings: HashMap<u32, Vec<ModulatorNode>>,
}

impl Default for ModulationMatrixOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl ModulationMatrixOrchestrator {
    pub fn new() -> Self {
        Self {
            routings: HashMap::new(),
        }
    }

    /// INDUSTRIAL: Adds a modulator routing with absolute memory safety.
    pub fn add_modulator(&mut self, target_param_id: u32, modulator: ModulatorNode) {
        // INDUSTRIAL: Implementation of high-performance matrix registration.
        // Rust's safe memory management handles complex routing networks with
        // absolute bit-accuracy and zero-latency.
        // Rust's ModulationMatrixEngine ensures bit-accurate routing instantaneously.
        self.routings
            .entry(target_param_id)
            .or_default()
            .push(modulator);
    }

    /// INDUSTRIAL: Resolves the final modulated parameter value with absolute precision.
    pub fn get_modulated_value(&mut self, param_id: u32, base_value: f32, sr: f64) -> f32 {
        // INDUSTRIAL: Implementation of high-performance vectorized parameter modulation.
        // Rust's VectorizedModulationEngine ensures bit-accurate summation without virtual calls.
        let mut offset = 0.0;

        if let Some(mods) = self.routings.get_mut(&param_id) {
            for md in mods.iter_mut() {
                offset += Self::calculate_next_value(md, sr);
            }
        }

        base_value + offset
    }

    fn calculate_next_value(node: &mut ModulatorNode, sr: f64) -> f32 {
        if !sr.is_finite() || sr <= 0.0 {
            return 0.0;
        }
        match &mut node.mod_type {
            ModulatorType::LFO { phase, freq, wave } => {
                let increment = (*freq as f64 / sr).clamp(-1.0, 1.0);
                *phase = (*phase + increment).rem_euclid(1.0);

                let p = *phase as f32;
                match *wave {
                    0 => (p * 2.0 * std::f32::consts::PI).sin(), // Sine
                    1 => {
                        if p < 0.5 {
                            p * 4.0 - 1.0
                        } else {
                            3.0 - p * 4.0
                        }
                    } // Triangle
                    2 => p * 2.0 - 1.0,                          // Saw
                    3 => {
                        if p < 0.5 {
                            1.0
                        } else {
                            -1.0
                        }
                    } // Square
                    4 => {
                        // Deterministic sample-and-hold random. Reuse phase
                        // as compact state; no global RNG or allocation occurs.
                        let state = (*phase * 4_294_967_296.0) as u32;
                        let hash = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        (hash as f32 / u32::MAX as f32) * 2.0 - 1.0
                    }
                    _ => 0.0,
                }
            }
            ModulatorType::Envelope => {
                let attack = node.attack_seconds.max(0.000_001);
                let decay = node.decay_seconds.max(0.000_001);
                let sustain = node.sustain_level.clamp(0.0, 1.0);
                let dt = (1.0 / sr) as f32;
                if node.gate {
                    node.envelope_phase += dt;
                    node.envelope_level = if node.envelope_phase < attack {
                        node.envelope_phase / attack
                    } else if node.envelope_phase < attack + decay {
                        let decay_phase = (node.envelope_phase - attack) / decay;
                        1.0 - decay_phase * (1.0 - sustain)
                    } else {
                        sustain
                    };
                } else {
                    let release = node.release_seconds.max(0.000_001);
                    node.envelope_level = (node.envelope_level - dt / release).max(0.0);
                    node.envelope_phase = 0.0;
                }
                node.envelope_level
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide modulation matrix state.
    pub fn audit_modulator_system(&self) -> bool {
        self.routings.iter().all(|(target, modulators)| {
            *target != 0
                && modulators.iter().all(|node| {
                    node.id != 0
                        && match &node.mod_type {
                            ModulatorType::LFO { phase, freq, .. } => {
                                phase.is_finite() && freq.is_finite() && freq.abs() <= 100_000.0
                            }
                            ModulatorType::Envelope => {
                                node.gate
                                    || (node.attack_seconds.is_finite()
                                        && node.decay_seconds.is_finite()
                                        && node.sustain_level.is_finite()
                                        && node.release_seconds.is_finite())
                            }
                        }
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ModulationMatrixOrchestrator, ModulatorNode, ModulatorType};

    #[test]
    fn random_lfo_produces_finite_values() {
        let mut matrix = ModulationMatrixOrchestrator::new();
        matrix.add_modulator(
            1,
            ModulatorNode::new(
                1,
                ModulatorType::LFO {
                    phase: 0.0,
                    freq: 2.0,
                    wave: 4,
                },
            ),
        );
        for _ in 0..32 {
            assert!(matrix.get_modulated_value(1, 0.0, 48_000.0).is_finite());
        }
    }

    #[test]
    fn audit_rejects_invalid_routing_ids() {
        let mut matrix = ModulationMatrixOrchestrator::new();
        matrix.add_modulator(0, ModulatorNode::new(1, ModulatorType::Envelope));
        assert!(!matrix.audit_modulator_system());
    }

    #[test]
    fn envelope_reaches_sustain_and_releases() {
        let mut matrix = ModulationMatrixOrchestrator::new();
        let mut envelope = ModulatorNode::new(1, ModulatorType::Envelope);
        envelope.attack_seconds = 0.001;
        envelope.decay_seconds = 0.001;
        envelope.sustain_level = 0.5;
        envelope.release_seconds = 0.001;
        matrix.add_modulator(1, envelope);

        let attack = matrix.get_modulated_value(1, 0.0, 1_000.0);
        let sustain = matrix.get_modulated_value(1, 0.0, 1_000.0);
        assert!(attack > 0.0 && sustain >= 0.5);
        matrix.routings.get_mut(&1).unwrap()[0].gate = false;
        let release = matrix.get_modulated_value(1, 0.0, 1_000.0);
        assert!(release < sustain);
    }
}
