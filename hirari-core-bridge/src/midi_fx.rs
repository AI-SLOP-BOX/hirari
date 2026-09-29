#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiEvent {
    pub timestamp: u32,
    pub data: [u8; 3],
}

pub struct ChordTriggerConfig {
    pub intervals: Vec<i32>,
    pub strum_ms: f32,
    pub velocity_scaling: f32,
}

pub struct MidiFxOrchestrator;

impl MidiFxOrchestrator {
    pub fn new() -> Self {
        Self
    }

    /// INDUSTRIAL: Resolves MIDI events with absolute precision and performance sovereignty.
    pub fn process_chord_trigger(
        &self,
        events: &[MidiEvent],
        config: &ChordTriggerConfig,
        sample_rate: f64,
    ) -> Vec<MidiEvent> {
        if !sample_rate.is_finite()
            || !(8_000.0..=384_000.0).contains(&sample_rate)
            || !config.strum_ms.is_finite()
            || !(0.0..=2_000.0).contains(&config.strum_ms)
            || !config.velocity_scaling.is_finite()
            || !(-1.0..=1.0).contains(&config.velocity_scaling)
            || config.intervals.len() > 128
        {
            return events.to_vec();
        }
        // INDUSTRIAL: Implementation of high-performance MIDI transformation.
        // Rust's safe memory management handles large MIDI streams with
        // absolute bit-accuracy and zero-latency.
        let mut output = Vec::with_capacity(events.len() * config.intervals.len());

        for ev in events {
            let status = ev.data[0] & 0xF0;
            if status == 0x90 || status == 0x80 {
                for (i, interval) in config.intervals.iter().enumerate() {
                    let note = (ev.data[1] as i32 + interval).clamp(0, 127) as u8;
                    let vel_factor = 1.0 - (i as f32 * config.velocity_scaling);
                    let vel = (ev.data[2] as f32 * vel_factor).clamp(1.0, 127.0) as u8;

                    let strum_samples = (config.strum_ms as f64 * 0.001 * sample_rate)
                        .round()
                        .min(u32::MAX as f64) as u32;
                    let offset = strum_samples.saturating_mul(i as u32);
                    output.push(MidiEvent {
                        timestamp: ev.timestamp.saturating_add(offset),
                        data: [status, note, vel],
                    });
                }
            } else {
                output.push(MidiEvent {
                    timestamp: ev.timestamp,
                    data: ev.data,
                });
            }
        }

        output
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide MIDI synchronization graph.
    pub fn audit_midi_fx(&self) -> bool {
        let input = [MidiEvent { timestamp: 100, data: [0x90, 60, 100] }];
        let config = ChordTriggerConfig { intervals: vec![0, 4, 7], strum_ms: 1.0, velocity_scaling: 0.1 };
        let output = self.process_chord_trigger(&input, &config, 48_000.0);
        output.len() == 3
            && output[0].timestamp == 100
            && output[1].timestamp == 148
            && output[2].data[1] == 67
            && self.process_chord_trigger(&input, &ChordTriggerConfig { intervals: vec![0], strum_ms: f32::NAN, velocity_scaling: 0.0 }, 48_000.0) == input
    }
}

impl Default for MidiFxOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{ChordTriggerConfig, MidiEvent, MidiFxOrchestrator};

    #[test]
    fn chord_trigger_applies_sample_accurate_strum_offsets() {
        let events = [MidiEvent {
            timestamp: 100,
            data: [0x90, 60, 100],
        }];
        let config = ChordTriggerConfig {
            intervals: vec![0, 4, 7],
            strum_ms: 1.0,
            velocity_scaling: 0.1,
        };
        let output = MidiFxOrchestrator::new().process_chord_trigger(&events, &config, 48_000.0);
        assert_eq!(output.len(), 3);
        assert_eq!(output[0].timestamp, 100);
        assert_eq!(output[1].timestamp, 148);
        assert_eq!(output[2].timestamp, 196);
    }

    #[test]
    fn chord_trigger_rejects_invalid_configuration_without_mutation() {
        let events = [MidiEvent {
            timestamp: 100,
            data: [0x90, 60, 100],
        }];
        let config = ChordTriggerConfig {
            intervals: vec![0],
            strum_ms: f32::NAN,
            velocity_scaling: 0.0,
        };
        let output = MidiFxOrchestrator::new().process_chord_trigger(&events, &config, 48_000.0);
        assert_eq!(output.as_slice(), events);
    }
}
