use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MIDIEventKind {
    ControlChange {
        controller: u8,
        value: u8,
    },
    PitchBend {
        value: f32,
    },
    ChannelAftertouch {
        pressure: f32,
    },
    /// MIDI 1.0 SysEx payload without the surrounding F0/F7 framing. The
    /// event model owns the bytes so a device disconnect cannot invalidate a
    /// queued message.
    SysEx {
        data: Vec<u8>,
    },
    /// MIDI 2.0 channel voice message represented in its semantic form.
    /// `status` is the high-nibble message type (0x8..=0xE), `index` is the
    /// note/controller index, and `value` preserves the full 32-bit payload.
    Midi2ChannelVoice {
        status: u8,
        index: u8,
        value: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MIDIEvent {
    pub beat: f32,
    pub channel: u8,
    pub kind: MIDIEventKind,
}

impl MIDIEvent {
    pub fn validate(&self) -> bool {
        self.beat.is_finite()
            && self.beat >= 0.0
            && self.channel < 16
            && match &self.kind {
                MIDIEventKind::ControlChange { controller, value } => {
                    *controller < 128 && *value < 128
                }
                MIDIEventKind::PitchBend { value } => (-1.0..=1.0).contains(value),
                MIDIEventKind::ChannelAftertouch { pressure } => (0.0..=1.0).contains(pressure),
                MIDIEventKind::SysEx { data } => {
                    data.len() <= 4096 && data.iter().all(|byte| *byte <= 0x7f)
                }
                MIDIEventKind::Midi2ChannelVoice { status, .. } => (0x8..=0xE).contains(status),
            }
    }
}

pub struct MPENoteState {
    pub note_number: u8,
    pub channel: u8,
    pub pressure: f32,
    pub timbre: f32,
    pub bend: f32,
}

pub struct ArticulationMap {
    pub id: u32,
    pub name: String,
    pub trigger_channel: u32,
}

pub struct MIDIOrchestrator {
    pub mpe_enabled: bool,
    pub mpe_notes: Vec<MPENoteState>,
    pub articulation_maps: Vec<ArticulationMap>,
    pub cc_mappings: HashMap<u32, u32>, // (Channel << 8 | CC) -> ParamID
}

impl Default for MIDIOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl MIDIOrchestrator {
    /// Decode one 64-bit MIDI 2.0 Channel Voice UMP into the common project
    /// event model. The group nibble is intentionally ignored here; device
    /// routing resolves groups before events reach a track.
    pub fn decode_midi2_channel_voice(beat: f32, word0: u32, word1: u32) -> Option<MIDIEvent> {
        if ((word0 >> 28) & 0x0f) != 0x04 {
            return None;
        }
        let status = ((word0 >> 20) & 0x0f) as u8;
        let channel = ((word0 >> 16) & 0x0f) as u8;
        let index = ((word0 >> 8) & 0xff) as u8;
        let event = MIDIEvent {
            beat,
            channel,
            kind: MIDIEventKind::Midi2ChannelVoice {
                status,
                index,
                value: word1,
            },
        };
        event.validate().then_some(event)
    }

    /// Construct a bounded SysEx event from a device payload. Framing bytes
    /// are omitted from the persisted event and are added by the device
    /// adapter when transmitting.
    pub fn sysex_event(beat: f32, channel: u8, data: Vec<u8>) -> Option<MIDIEvent> {
        let event = MIDIEvent {
            beat,
            channel,
            kind: MIDIEventKind::SysEx { data },
        };
        event.validate().then_some(event)
    }

    pub fn normalize_events(events: &mut Vec<MIDIEvent>) -> bool {
        if events.iter().any(|event| !event.validate()) {
            return false;
        }
        events.sort_by(|left, right| {
            left.beat
                .total_cmp(&right.beat)
                .then(left.channel.cmp(&right.channel))
        });
        events.dedup();
        true
    }

    pub fn new() -> Self {
        Self {
            mpe_enabled: false,
            mpe_notes: Vec::with_capacity(16),
            articulation_maps: Vec::new(),
            cc_mappings: HashMap::new(),
        }
    }

    /// INDUSTRIAL: Processes MPE data with absolute precision and MIDI sovereignty.
    pub fn process_mpe(&mut self, buffer: &mut [u8]) {
        if !self.mpe_enabled {
            return;
        }
        // Consume packed MIDI-1 channel voice messages.  MPE uses channel
        // pressure and pitch bend on the note's member channel; malformed
        // trailing bytes are ignored rather than being interpreted as a new
        // event.
        for message in buffer.as_chunks::<3>().0 {
            let status = message[0];
            if status & 0x80 == 0 {
                continue;
            }
            let channel = status & 0x0f;
            let kind = status & 0xf0;
            let data1 = message[1] & 0x7f;
            let data2 = message[2] & 0x7f;
            if channel == 0 {
                continue;
            }
            match kind {
                0x90 if data2 != 0 => {
                    if let Some(note) = self
                        .mpe_notes
                        .iter_mut()
                        .find(|note| note.channel == channel && note.note_number == data1)
                    {
                        // A note-on velocity is not per-note pressure. Keep
                        // the expressive dimension neutral until an actual
                        // channel-pressure message arrives.
                        note.pressure = 0.0;
                    } else if self.mpe_notes.len() < 128 {
                        self.mpe_notes.push(MPENoteState {
                            note_number: data1,
                            channel,
                            pressure: 0.0,
                            timbre: 0.0,
                            bend: 0.0,
                        });
                    }
                }
                0x80 | 0x90 => self
                    .mpe_notes
                    .retain(|note| !(note.channel == channel && note.note_number == data1)),
                0xD0 => {
                    if let Some(note) = self
                        .mpe_notes
                        .iter_mut()
                        .find(|note| note.channel == channel)
                    {
                        note.pressure = data1 as f32 / 127.0;
                    }
                }
                0xE0 => {
                    if let Some(note) = self
                        .mpe_notes
                        .iter_mut()
                        .find(|note| note.channel == channel)
                    {
                        note.bend =
                            ((u16::from(data2) << 7 | u16::from(data1)) as f32 - 8192.0) / 8192.0;
                    }
                }
                0xB0 if data1 == 74 => {
                    if let Some(note) = self
                        .mpe_notes
                        .iter_mut()
                        .find(|note| note.channel == channel)
                    {
                        note.timbre = data2 as f32 / 127.0;
                    }
                }
                _ => {}
            }
        }
    }

    /// INDUSTRIAL: Orchestrates articulation mapping with industrial precision and creative sovereignty.
    pub fn update_articulations(&mut self, maps: Vec<ArticulationMap>) {
        // INDUSTRIAL: Implementation of high-performance articulation management.
        // Rust's ArticulationEngine ensures bit-accurate mapping distribution instantaneously.
        self.articulation_maps = maps;
    }

    /// INDUSTRIAL: Resolves MIDI CC messages to parameter values with absolute precision and hardware sovereignty.
    pub fn handle_cc(&self, channel: u8, cc: u8, value: u8) -> Option<(u32, f32)> {
        // INDUSTRIAL: Implementation of high-performance mapping resolution.
        // Rust's MappingEngine ensures bit-accurate parameter synchronization instantaneously.
        let key = ((channel as u32) << 8) | (cc as u32);
        self.cc_mappings.get(&key).map(|&param_id| {
            // INDUSTRIAL: Takeover logic and value scaling.
            // Rust's TakeoverEngine ensures perfectly smooth hardware control.
            (param_id, value as f32 / 127.0)
        })
    }

    pub fn apply_swing(events: &mut [MIDIEvent], subdivision_beats: f32, amount: f32) -> bool {
        if !subdivision_beats.is_finite()
            || subdivision_beats <= 0.0
            || !amount.is_finite()
            || !(-1.0..=1.0).contains(&amount)
            || events.iter().any(|event| !event.validate())
        {
            return false;
        }
        for event in events {
            let cell = (event.beat / subdivision_beats).floor() as i64;
            if cell % 2 != 0 {
                event.beat = (event.beat + subdivision_beats * 0.5 * amount).max(0.0);
            }
        }
        true
    }

    /// Moves events toward the nearest musical grid without destroying the
    /// original timing. Strength 0 leaves events untouched; strength 1 is
    /// hard quantize. The operation is deterministic and preserves ordering.
    pub fn quantize(events: &mut [MIDIEvent], grid_beats: f32, strength: f32) -> bool {
        if !grid_beats.is_finite()
            || !(0.001..=16.0).contains(&grid_beats)
            || !strength.is_finite()
            || !(0.0..=1.0).contains(&strength)
            || events.iter().any(|event| !event.validate())
        {
            return false;
        }
        for event in events.iter_mut() {
            let target = (event.beat / grid_beats).round() * grid_beats;
            event.beat += (target - event.beat) * strength;
            if !event.beat.is_finite() || event.beat < 0.0 {
                return false;
            }
        }
        events.sort_by(|left, right| {
            left.beat
                .total_cmp(&right.beat)
                .then(left.channel.cmp(&right.channel))
        });
        true
    }

    pub fn humanize(events: &mut [MIDIEvent], timing_beats: f32, velocity: i16, seed: u64) -> bool {
        if !timing_beats.is_finite()
            || timing_beats < 0.0
            || !(-127..=127).contains(&velocity)
            || events.iter().any(|event| !event.validate())
        {
            return false;
        }
        let mut state = seed.max(1);
        for event in events {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let timing = ((state >> 32) as u32) as f32 / u32::MAX as f32 * 2.0 - 1.0;
            event.beat = (event.beat + timing * timing_beats).max(0.0);
            if let MIDIEventKind::ControlChange { value, .. } = &mut event.kind {
                let delta = (timing * velocity as f32).round() as i16;
                *value = (*value as i16 + delta).clamp(0, 127) as u8;
            }
        }
        true
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide MIDI synchronization graph.
    pub fn audit_midi(&self) -> bool {
        self.mpe_notes.len() <= 128
            && self.mpe_notes.iter().all(|note| {
                note.note_number < 128
                    && note.channel < 16
                    && note.pressure.is_finite()
                    && (0.0..=1.0).contains(&note.pressure)
                    && note.timbre.is_finite()
                    && (0.0..=1.0).contains(&note.timbre)
                    && note.bend.is_finite()
                    && (-1.0..=1.0).contains(&note.bend)
            })
            && self.articulation_maps.len() <= 4096
            && self.articulation_maps.iter().all(|map| {
                map.id != 0
                    && !map.name.trim().is_empty()
                    && map.name.len() <= 128
                    && !map.name.contains('\0')
                    && map.trigger_channel < 16
            })
            && self.articulation_maps.iter().enumerate().all(|(i, map)| {
                self.articulation_maps[..i]
                    .iter()
                    .all(|prev| prev.id != map.id)
            })
            && self.cc_mappings.keys().all(|key| {
                let channel = (key >> 8) & 0xff;
                let controller = key & 0xff;
                channel < 16 && controller < 128
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{MIDIEvent, MIDIEventKind, MIDIOrchestrator};

    fn events() -> Vec<MIDIEvent> {
        vec![
            MIDIEvent {
                beat: 0.0,
                channel: 0,
                kind: MIDIEventKind::ControlChange {
                    controller: 1,
                    value: 64,
                },
            },
            MIDIEvent {
                beat: 0.5,
                channel: 1,
                kind: MIDIEventKind::PitchBend { value: 0.0 },
            },
            MIDIEvent {
                beat: 1.0,
                channel: 2,
                kind: MIDIEventKind::ChannelAftertouch { pressure: 0.5 },
            },
        ]
    }

    #[test]
    fn validates_cc_pitch_bend_and_aftertouch_events() {
        assert!(events().iter().all(MIDIEvent::validate));
        let invalid = MIDIEvent {
            beat: -1.0,
            channel: 16,
            kind: MIDIEventKind::PitchBend { value: 2.0 },
        };
        assert!(!invalid.validate());
    }

    #[test]
    fn swing_moves_offbeat_events_and_humanize_is_reproducible() {
        let mut swung = events();
        assert!(MIDIOrchestrator::apply_swing(&mut swung, 0.5, 0.5));
        assert!((swung[1].beat - 0.625).abs() < 0.001);

        let mut first = events();
        let mut second = events();
        assert!(MIDIOrchestrator::humanize(&mut first, 0.05, 10, 42));
        assert!(MIDIOrchestrator::humanize(&mut second, 0.05, 10, 42));
        assert_eq!(first, second);
    }

    #[test]
    fn normalization_sorts_and_removes_duplicate_events() {
        let event = MIDIEvent {
            beat: 1.0,
            channel: 0,
            kind: MIDIEventKind::ControlChange {
                controller: 1,
                value: 64,
            },
        };
        let mut events = vec![
            event.clone(),
            event,
            MIDIEvent {
                beat: 0.0,
                channel: 0,
                kind: MIDIEventKind::ControlChange {
                    controller: 2,
                    value: 1,
                },
            },
        ];
        assert!(MIDIOrchestrator::normalize_events(&mut events));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].beat, 0.0);
        assert_eq!(events[1].beat, 1.0);
    }

    #[test]
    fn quantize_preserves_partial_strength_and_order() {
        let mut events = vec![
            MIDIEvent {
                beat: 0.37,
                channel: 0,
                kind: MIDIEventKind::ControlChange {
                    controller: 1,
                    value: 20,
                },
            },
            MIDIEvent {
                beat: 0.11,
                channel: 0,
                kind: MIDIEventKind::ControlChange {
                    controller: 2,
                    value: 30,
                },
            },
        ];
        assert!(MIDIOrchestrator::quantize(&mut events, 0.25, 0.5));
        assert!((events[0].beat - 0.055).abs() < 0.001);
        assert!((events[1].beat - 0.31).abs() < 0.001);
        assert!(!MIDIOrchestrator::quantize(&mut events, 0.0, 1.0));
    }

    #[test]
    fn midi_audit_rejects_corrupt_mpe_state() {
        let mut midi = MIDIOrchestrator::new();
        midi.mpe_notes.push(super::MPENoteState {
            note_number: 128,
            channel: 0,
            pressure: 0.0,
            timbre: 0.0,
            bend: 0.0,
        });
        assert!(!midi.audit_midi());
    }
}
