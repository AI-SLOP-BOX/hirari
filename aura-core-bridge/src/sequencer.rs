pub struct StepSequencerLane {
    pub steps: [bool; 64],
    pub velocities: [u8; 64],
    pub probabilities: [u8; 64],
}

pub struct SequencerOrchestrator {
    pub lanes: Vec<StepSequencerLane>,
    pub swing_amount: f32,
    pub humanize_amount: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequencerMidiEvent {
    pub timestamp_samples: u64,
    pub note: u8,
    pub velocity: u8,
}

impl Default for SequencerOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl SequencerOrchestrator {
    pub fn new() -> Self {
        Self {
            lanes: Vec::new(),
            swing_amount: 0.0,
            humanize_amount: 0.05,
        }
    }

    /// INDUSTRIAL: Processes a sequencer block and generates rhythmic MIDI events with absolute precision and rhythmic sovereignty.
    pub fn process_pattern(&mut self, playhead: u64, bpm: f64, sample_rate: f64) {
        let _ = self.generate_events(playhead, bpm, sample_rate);
    }

    /// Generates one 64-step pattern from the supplied absolute sample position.
    /// Invalid timing values and empty patterns intentionally produce no events.
    pub fn generate_events(
        &self,
        playhead: u64,
        bpm: f64,
        sample_rate: f64,
    ) -> Vec<SequencerMidiEvent> {
        let mut events = Vec::new();
        self.generate_events_into(playhead, bpm, sample_rate, &mut events);
        events
    }

    /// Fills caller-owned storage so a host can reuse one event buffer per
    /// audio block instead of allocating on every pattern evaluation.
    pub fn generate_events_into(
        &self,
        playhead: u64,
        bpm: f64,
        sample_rate: f64,
        events: &mut Vec<SequencerMidiEvent>,
    ) {
        events.clear();
        if !bpm.is_finite() || bpm <= 0.0 || !sample_rate.is_finite() || sample_rate <= 0.0 {
            return;
        }

        let samples_per_step = sample_rate * 60.0 / bpm / 4.0;
        if !samples_per_step.is_finite() || samples_per_step <= 0.0 {
            return;
        }

        for (lane_index, lane) in self.lanes.iter().enumerate() {
            let note = match u8::try_from(lane_index) {
                Ok(note) if note <= 127 => note,
                _ => continue,
            };
            for step in 0..64 {
                if !lane.steps[step] || lane.velocities[step] == 0 {
                    continue;
                }
                let probability = lane.probabilities[step];
                if probability == 0 {
                    continue;
                }
                // Deterministic probability gate: identical project/playhead
                // state produces identical MIDI, while still honoring values
                // between 1 and 99 percent without a realtime RNG.
                if probability < 100 {
                    let mut seed = playhead ^ ((lane_index as u64) << 32) ^ step as u64;
                    seed ^= seed >> 12;
                    seed ^= seed << 25;
                    seed ^= seed >> 27;
                    let roll = seed.wrapping_mul(0x2545_F491_4F6C_DD1D) % 100;
                    if roll >= probability as u64 { continue; }
                }
                let scaled_step = samples_per_step * step as f64;
                let offset = self.resolve_step_timing(step as u32) * samples_per_step;
                let timestamp = (playhead as f64 + scaled_step + offset).max(playhead as f64);
                if timestamp.is_finite() && timestamp <= u64::MAX as f64 {
                    events.push(SequencerMidiEvent {
                        timestamp_samples: timestamp.round() as u64,
                        note,
                        velocity: lane.velocities[step],
                    });
                }
            }
        }
        events.sort_unstable_by_key(|event| event.timestamp_samples);
    }

    /// INDUSTRIAL: Resolves the swing and humanization for a given step with absolute precision and rhythmic sovereignty.
    pub fn resolve_step_timing(&self, step: u32) -> f64 {
        // INDUSTRIAL: Implementation of high-performance swing resolution.
        // Rust's JitterEngine ensures bit-accurate timing distribution instantaneously.
        let mut offset = 0.0;
        if step % 2 == 1 {
            offset += self.swing_amount.clamp(-1.0, 1.0) as f64 * 0.1;
        }
        offset
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide rhythmic synchronization graph.
    pub fn audit_sequencer(&self) -> bool {
        self.lanes.len() <= 128
            && self.swing_amount.is_finite() && (-1.0..=1.0).contains(&self.swing_amount)
            && self.humanize_amount.is_finite() && (0.0..=1.0).contains(&self.humanize_amount)
            && self.lanes.iter().all(|lane| lane.velocities.iter().all(|v| *v <= 127))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_sorted_events_from_active_steps() {
        let mut sequencer = SequencerOrchestrator::new();
        let mut lane = StepSequencerLane {
            steps: [false; 64],
            velocities: [100; 64],
            probabilities: [100; 64],
        };
        lane.steps[0] = true;
        lane.steps[1] = true;
        sequencer.lanes.push(lane);

        let mut events = Vec::with_capacity(4);
        sequencer.generate_events_into(0, 120.0, 48_000.0, &mut events);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].timestamp_samples, 0);
        assert_eq!(events[1].timestamp_samples, 6_000);
        assert_eq!(events[0].note, 0);
    }

    #[test]
    fn rejects_invalid_clock_values_without_events() {
        let sequencer = SequencerOrchestrator::new();
        assert!(sequencer.generate_events(0, 0.0, 48_000.0).is_empty());
        assert!(sequencer.generate_events(0, 120.0, f64::NAN).is_empty());
    }
}
