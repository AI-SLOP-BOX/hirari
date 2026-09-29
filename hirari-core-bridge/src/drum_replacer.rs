use crate::transient::TransientOrchestrator;

#[derive(Clone, Debug)]
pub struct TriggerEvent {
    pub sample_position: u64,
    pub velocity: f32,
    pub midi_note: u8,
}

pub struct DrumReplacerEngine {}

impl Default for DrumReplacerEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl DrumReplacerEngine {
    pub fn new() -> Self {
        Self {}
    }

    /**
     * @brief CONVERT: Converts transient peaks into TriggerEvents.
     * INDUSTRIAL: Re-scales transient strengths with realistic velocity response curve.
     */
    pub fn convert_to_midi(
        &self,
        buffer: &[f32],
        sample_rate: f64,
        target_note: u8,
    ) -> Vec<TriggerEvent> {
        self.convert_to_midi_with_config(buffer, sample_rate, target_note, 0.4, 512)
    }

    /// Configurable trigger profile for kick/snare replacement workflows.
    /// `sensitivity` is normalized 0..=1 and `retrigger_samples` suppresses
    /// accidental double hits within the requested sample window.
    pub fn convert_to_midi_with_config(
        &self,
        buffer: &[f32],
        sample_rate: f64,
        target_note: u8,
        sensitivity: f64,
        retrigger_samples: u32,
    ) -> Vec<TriggerEvent> {
        if !sample_rate.is_finite()
            || !(8_000.0..=384_000.0).contains(&sample_rate)
            || !sensitivity.is_finite()
            || !(0.0..=1.0).contains(&sensitivity)
        {
            return Vec::new();
        }
        let mut detector = TransientOrchestrator::new(sample_rate);
        let peaks = detector.analyze_transients(buffer, sensitivity as f32);

        let mut triggers = Vec::new();
        let mut last_position: Option<u64> = None;
        for p in peaks {
            let idx = p.sample_index as usize;
            if idx < buffer.len() {
                if last_position.is_some_and(|last| {
                    p.sample_index < last.saturating_add(u64::from(retrigger_samples))
                }) {
                    continue;
                }
                let peak_val = buffer[idx].abs();
                let vel = (peak_val * 127.0).clamp(1.0, 127.0);
                triggers.push(TriggerEvent {
                    sample_position: p.sample_index,
                    velocity: vel,
                    midi_note: target_note,
                });
                last_position = Some(p.sample_index);
            }
        }

        triggers
    }

    /**
     * @brief STREAM: Generates a sorted, high-fidelity MIDI byte stream with Note On / Note Off pairs.
     * INDUSTRIAL:
     *  - Generates a Note Off event for each Note On after a 2048-sample gate length (~46ms).
     *  - Interleaves and sorts all events chronologically to guarantee correct temporal order.
     *  - Outputs an 8-byte aligned packet layout:
     *    [offset (4 bytes), status (1 byte), note (1 byte), velocity (1 byte), padding (1 byte)]
     */
    pub fn generate_midi_stream(&self, triggers: &[TriggerEvent]) -> Vec<u8> {
        struct InternalMidiEvent {
            offset: u32,
            status: u8,
            note: u8,
            velocity: u8,
        }

        let mut events = Vec::with_capacity(triggers.len() * 2);
        let gate_length = 2048u32; // ~46ms gate at 44.1k

        for t in triggers {
            let pos = t.sample_position as u32;

            // Note ON
            events.push(InternalMidiEvent {
                offset: pos,
                status: 0x90,
                note: t.midi_note.min(127),
                velocity: t.velocity.clamp(1.0, 127.0).round() as u8,
            });
            // Note OFF scheduled later
            events.push(InternalMidiEvent {
                offset: pos.saturating_add(gate_length),
                status: 0x80,
                note: t.midi_note.min(127),
                velocity: 0,
            });
        }

        // Sort all events chronologically by absolute sample offset
        events.sort_by_key(|e| e.offset);

        // Package into 8-byte aligned binary packets for safe cross-FFI ingestion
        let mut stream = Vec::with_capacity(events.len() * 8);
        for e in events {
            let offset_bytes = e.offset.to_le_bytes();
            stream.extend_from_slice(&offset_bytes);
            stream.push(e.status);
            stream.push(e.note);
            stream.push(e.velocity);
            stream.push(0); // Alignment padding byte
        }
        stream
    }

    pub fn audit_drum_replacer(&self) -> bool {
        self.convert_to_midi(&[], 48_000.0, 36).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{DrumReplacerEngine, TriggerEvent};

    #[test]
    fn configurable_profile_rejects_invalid_sensitivity() {
        let engine = DrumReplacerEngine::new();
        let samples = [0.0_f32, 1.0, 0.0];
        assert!(engine
            .convert_to_midi_with_config(&samples, 48_000.0, 36, f32::NAN.into(), 512)
            .is_empty());
        assert!(engine
            .convert_to_midi_with_config(&samples, 48_000.0, 36, 1.5, 512)
            .is_empty());
        assert!(engine
            .convert_to_midi_with_config(&samples, f64::NAN, 36, 0.4, 512)
            .is_empty());
    }

    #[test]
    fn midi_stream_preserves_trigger_order_and_gate_pairs() {
        let engine = DrumReplacerEngine::new();
        let stream = engine.generate_midi_stream(&[
            TriggerEvent {
                sample_position: 100,
                velocity: 120.0,
                midi_note: 36,
            },
            TriggerEvent {
                sample_position: 200,
                velocity: 80.0,
                midi_note: 38,
            },
        ]);
        assert_eq!(stream.len(), 32);
        assert_eq!(&stream[0..4], &100u32.to_le_bytes());
        assert_eq!(&stream[4..7], &[0x90, 36, 120]);
        assert_eq!(&stream[12..15], &[0x90, 38, 80]);
        assert_eq!(&stream[20..23], &[0x80, 36, 0]);
        assert_eq!(&stream[28..31], &[0x80, 38, 0]);
    }

    #[test]
    fn midi_stream_clamps_note_and_velocity_to_midi7bit() {
        let engine = DrumReplacerEngine::new();
        let stream = engine.generate_midi_stream(&[TriggerEvent {
            sample_position: 0,
            velocity: 240.0,
            midi_note: 200,
        }]);
        assert_eq!(&stream[4..7], &[0x90, 127, 127]);
        assert_eq!(&stream[12..15], &[0x80, 127, 0]);
    }
}
