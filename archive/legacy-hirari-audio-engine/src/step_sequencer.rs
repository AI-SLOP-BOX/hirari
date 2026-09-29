use crate::{MidiEvent, MidiEventType};
use parking_lot::Mutex;

pub struct StepSequencer {
    pub patterns: Mutex<std::collections::HashMap<u64, Vec<u8>>>,
    pub current_step: Mutex<usize>,
    pub step_duration_samples: u32,
    pub samples_since_last_step: Mutex<u32>,
}

impl StepSequencer {
    pub fn new(sample_rate: u32, bpm: f32) -> Self {
        let step_duration = if sample_rate > 0 && bpm.is_finite() && bpm > 0.0 {
            (sample_rate as f32 * 60.0 / bpm / 4.0)
                .round()
                .clamp(1.0, u32::MAX as f32) as u32
        } else {
            1
        };
        Self {
            patterns: Mutex::new(std::collections::HashMap::new()),
            current_step: Mutex::new(0),
            step_duration_samples: step_duration,
            samples_since_last_step: Mutex::new(0),
        }
    }
    pub fn process_tick(&self, samples: u32) -> Vec<MidiEvent> {
        let mut events = Vec::new();
        let mut step_count = self.samples_since_last_step.lock();
        *step_count = step_count.saturating_add(samples);
        if *step_count >= self.step_duration_samples {
            *step_count -= self.step_duration_samples;
            let mut curr = self.current_step.lock();
            let patterns = self.patterns.lock();
            for (&node_id, steps) in patterns.iter() {
                if steps.is_empty() {
                    continue;
                }
                let velocity = steps[*curr % steps.len()];
                if velocity > 0 {
                    events.push(MidiEvent {
                        timestamp_samples: 0,
                        channel: 0,
                        event: MidiEventType::NoteOn { note: (node_id % 127) as u8, velocity },
                    });
                }
            }
            *curr = (*curr + 1) % 16;
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_timing_uses_safe_single_sample_step() {
        assert_eq!(StepSequencer::new(0, 0.0).step_duration_samples, 1);
        assert_eq!(StepSequencer::new(48_000, f32::NAN).step_duration_samples, 1);
    }

    #[test]
    fn empty_pattern_does_not_panic() {
        let sequencer = StepSequencer::new(48_000, 120.0);
        sequencer.patterns.lock().insert(1, Vec::new());
        assert!(sequencer.process_tick(u32::MAX).is_empty());
    }
}
