use crate::{MidiEvent, MidiEventType};
use parking_lot::Mutex;

/// Industrial Step Sequencer [Nodal Pattern Generation]
/// A functional implementation for generating MIDI sequences based on a grid pattern.
pub struct StepSequencer {
    pub patterns: Mutex<std::collections::HashMap<u64, Vec<u8>>>, // NodeID -> 16 Steps
    pub note_map: Mutex<std::collections::HashMap<u64, u8>>, // NodeID -> explicit MIDI note
    pub current_step: Mutex<usize>,
    pub step_duration_samples: u32,
    pub samples_since_last_step: Mutex<u32>,
}

impl StepSequencer {
    const MAX_EVENTS_PER_STEP: usize = 128;

    pub fn new(sample_rate: u32, bpm: f32) -> Self {
        let step_duration = if sample_rate > 0 && bpm.is_finite() && bpm > 0.0 {
            (sample_rate as f64 * 60.0 / bpm as f64 / 4.0)
                .ceil()
                .min(u32::MAX as f64) as u32
        } else {
            0
        }
        .max(1); // 16th notes; keep the duration usable for invalid input too
        Self {
            patterns: Mutex::new(std::collections::HashMap::new()),
            note_map: Mutex::new(std::collections::HashMap::new()),
            current_step: Mutex::new(0),
            step_duration_samples: step_duration,
            samples_since_last_step: Mutex::new(0),
        }
    }

    pub fn process_tick(&self, samples: u32) -> Vec<MidiEvent> {
        let mut events = Vec::with_capacity(Self::MAX_EVENTS_PER_STEP);
        self.process_tick_into(samples, &mut events);
        events
    }

    /// Generate events into caller-owned storage. The caller can reserve once
    /// and reuse the same Vec for every audio block, avoiding a heap allocation
    /// in the sequencing path.
    pub fn process_tick_into(&self, samples: u32, events: &mut Vec<MidiEvent>) {
        events.clear();
        let mut step_count = self.samples_since_last_step.lock();
        let mut remaining = samples;
        let mut elapsed = 0u32;
        if *step_count >= self.step_duration_samples {
            *step_count %= self.step_duration_samples;
        }

        // Consume every step boundary inside the block. The previous
        // implementation advanced at most once, dropping events whenever the
        // host supplied a block larger than one step.
        while remaining > 0 {
            let until_step = self.step_duration_samples.saturating_sub(*step_count);
            if remaining < until_step {
                *step_count += remaining;
                break;
            }

            remaining -= until_step;
            elapsed = elapsed.saturating_add(until_step);
            *step_count = 0;

            let curr = *self.current_step.lock();
            if events.len() < Self::MAX_EVENTS_PER_STEP {
                let patterns = self.patterns.lock();
                let note_map = self.note_map.lock();
                for (&node_id, steps) in patterns.iter() {
                    if events.len() >= Self::MAX_EVENTS_PER_STEP || steps.is_empty() {
                        break;
                    }
                    let velocity = steps[curr % steps.len()];
                    if velocity > 0 {
                        events.push(MidiEvent {
                            timestamp_samples: elapsed.min(samples),
                            channel: 0,
                            event: MidiEventType::NoteOn {
                                note: note_map.get(&node_id).copied().unwrap_or(60),
                                velocity,
                            },
                        });
                    }
                }
            }

            let mut current = self.current_step.lock();
            *current = (*current + 1) % 16;
        }
    }

    pub fn set_pattern(&self, node_id: u64, steps: Vec<u8>) {
        if steps.is_empty() {
            self.patterns.lock().remove(&node_id);
            return;
        }
        self.patterns.lock().insert(node_id, steps);
    }

    pub fn set_note(&self, node_id: u64, note: u8) {
        self.note_map.lock().insert(node_id, note.min(127));
    }
}

#[cfg(test)]
mod tests {
    use super::StepSequencer;

    #[test]
    fn large_block_emits_each_step_boundary() {
        let sequencer = StepSequencer::new(16, 60.0);
        sequencer.set_pattern(1, vec![100; 16]);
        sequencer.set_note(1, 36);

        let mut events = Vec::with_capacity(8);
        sequencer.process_tick_into(10, &mut events);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].timestamp_samples, 4);
        assert_eq!(events[1].timestamp_samples, 8);

        events.clear();
        sequencer.process_tick_into(2, &mut events);
        assert!(events.is_empty());

        sequencer.process_tick_into(2, &mut events);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].timestamp_samples, 2);
    }
}
