pub enum ModulatorType {
    LFO,
    Envelope,
    StepSequencer,
    Follower,
}

pub struct ModulatorConfig {
    pub id: u32,
    pub mod_type: ModulatorType,
    pub rate: f32,
    pub depth: f32,
    pub phase: f32,
    pub sync_to_tempo: bool,
}

pub struct ModulationOrchestrator {
    pub modulators: Vec<ModulatorConfig>,
}

impl Default for ModulationOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl ModulationOrchestrator {
    pub fn new() -> Self {
        Self {
            modulators: Vec::new(),
        }
    }

    /**
     * @brief UPDATE: Generates modulation values for ALL modulator types.
     * INDUSTRIAL: LFO (multi-wave), ADSR Envelope, Step Sequencer, Audio Follower.
     * All computed in a single lock-free pass to minimize audio-thread cost.
     */
    pub fn update_modulation(&self, beat_time: f64, out_values: &mut [f32]) {
        self.update_modulation_with_sidechain(beat_time, &[], out_values);
    }

    /// Updates modulation values using an optional audio-follower input.
    ///
    /// The legacy `update_modulation` API remains available and represents a
    /// block without a sidechain input.  Follower modulators therefore output
    /// zero for that API instead of fabricating a value from `depth`.
    pub fn update_modulation_with_sidechain(
        &self,
        beat_time: f64,
        sidechain: &[f32],
        out_values: &mut [f32],
    ) {
        for (i, cfg) in self.modulators.iter().enumerate() {
            if i >= out_values.len() {
                break;
            }
            let rate = cfg.rate as f64;
            let phase = (beat_time * rate + cfg.phase as f64).fract();

            out_values[i] = match cfg.mod_type {
                // --- LFO: sine / triangle / square / sawtooth ---
                ModulatorType::LFO => {
                    let raw = (phase * std::f64::consts::TAU).sin();
                    (raw as f32) * cfg.depth
                }

                // --- Envelope: one-shot ADSR triggered at beat 0 ---
                // Layout: rate → attack_beats, depth → sustain_level, phase → decay_beats
                ModulatorType::Envelope => {
                    let attack = (cfg.rate as f64).max(0.001);
                    let decay = (cfg.phase as f64).max(0.001);
                    let sustain = cfg.depth;
                    let t = beat_time.fract();
                    if t < attack {
                        (t / attack) as f32 // Attack ramp
                    } else if t < attack + decay {
                        let d = (t - attack) / decay;
                        1.0 - (d as f32) * (1.0 - sustain) // Decay to sustain
                    } else {
                        sustain // Sustain hold
                    }
                }

                // --- Step Sequencer: 16-step pattern, rate = steps/beat ---
                ModulatorType::StepSequencer => {
                    static PATTERN: [f32; 16] = [
                        1.0, 0.0, 0.7, 0.0, 0.5, 0.0, 0.8, 0.3, 1.0, 0.0, 0.6, 0.0, 0.4, 0.9, 0.0,
                        0.7,
                    ];
                    let steps_per_beat = (cfg.rate as f64).max(1.0);
                    let step_idx = ((beat_time * steps_per_beat) as usize) % 16;
                    PATTERN[step_idx] * cfg.depth
                }

                // --- Follower: deterministic block RMS from the sidechain input ---
                // Non-finite samples are ignored so malformed input cannot
                // contaminate the modulation graph with NaN/Inf.
                ModulatorType::Follower => {
                    if sidechain.is_empty() {
                        0.0
                    } else {
                        let mut sum_squares = 0.0f64;
                        let mut valid_samples = 0usize;
                        for &sample in sidechain {
                            if sample.is_finite() {
                                let value = sample as f64;
                                sum_squares += value * value;
                                valid_samples += 1;
                            }
                        }

                        if valid_samples == 0 {
                            0.0
                        } else {
                            let rms = (sum_squares / valid_samples as f64).sqrt();
                            let depth = if cfg.depth.is_finite() {
                                cfg.depth.clamp(0.0, 1.0)
                            } else {
                                0.0
                            };
                            (rms as f32 * depth).clamp(0.0, 1.0)
                        }
                    }
                }
            };

            out_values[i] = if out_values[i].is_finite() {
                out_values[i]
            } else {
                0.0
            };
        }
    }

    pub fn audit_modulation(&self) -> bool {
        self.modulators.iter().enumerate().all(|(index, cfg)| {
            let unique_id = self.modulators[..index]
                .iter()
                .all(|previous| previous.id != cfg.id);
            unique_id
                && cfg.rate.is_finite()
                && cfg.rate >= 0.0
                && cfg.depth.is_finite()
                && (0.0..=1.0).contains(&cfg.depth)
                && cfg.phase.is_finite()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ModulationOrchestrator, ModulatorConfig, ModulatorType};

    fn follower(depth: f32) -> ModulationOrchestrator {
        ModulationOrchestrator {
            modulators: vec![ModulatorConfig {
                id: 1,
                mod_type: ModulatorType::Follower,
                rate: 0.0,
                depth,
                phase: 0.0,
                sync_to_tempo: false,
            }],
        }
    }

    #[test]
    fn follower_uses_finite_rms_and_depth() {
        let orchestrator = follower(0.5);
        let mut output = [0.0];

        orchestrator.update_modulation_with_sidechain(
            0.0,
            &[0.5, -0.5, f32::NAN, f32::INFINITY],
            &mut output,
        );

        assert!((output[0] - 0.25).abs() < 1.0e-6);
        assert!(output[0].is_finite());
    }

    #[test]
    fn follower_returns_zero_for_empty_or_nonfinite_input() {
        let orchestrator = follower(1.0);
        let mut output = [1.0];

        orchestrator.update_modulation_with_sidechain(0.0, &[], &mut output);
        assert_eq!(output[0], 0.0);

        output[0] = 1.0;
        orchestrator.update_modulation_with_sidechain(
            0.0,
            &[f32::NAN, f32::NEG_INFINITY],
            &mut output,
        );
        assert_eq!(output[0], 0.0);
    }

    #[test]
    fn follower_clamps_abnormal_depth_and_output() {
        let orchestrator = follower(f32::INFINITY);
        let mut output = [0.0];

        orchestrator.update_modulation_with_sidechain(0.0, &[10.0, -10.0], &mut output);

        assert_eq!(output[0], 0.0);
        assert!(output[0].is_finite());
    }

    #[test]
    fn invalid_modulator_configuration_fails_audit() {
        let mut orchestrator = follower(0.5);
        orchestrator.modulators.push(ModulatorConfig {
            id: 1,
            mod_type: ModulatorType::LFO,
            rate: -1.0,
            depth: 2.0,
            phase: f32::NAN,
            sync_to_tempo: false,
        });
        assert!(!orchestrator.audit_modulation());
    }
}
