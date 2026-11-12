//! CoreMIDI polling and CC-learn bindings for the AmeUI MIDI LEARN card.
//!
//! The engine owns the actual device connection (macOS CoreMIDI via the
//! bridge's `start_midi_input` / `poll_midi_input_json`); this module owns
//! the packet decoding, the running-status table, the 30-second retrospective
//! buffer and the learn flow. It is the AmeUI counterpart of the old
//! Slint `ui/midi_input.rs` and renders identically on every macOS host.

use hirari_core_bridge::{project_contracts::MidiNoteContract, HirariCore};
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

pub(crate) const RETROSPECTIVE_MIDI_SECONDS: u64 = 30;
pub(crate) const MAX_RETROSPECTIVE_MIDI_EVENTS: usize = 8192;

#[derive(Clone, Copy, Debug)]
pub(crate) enum MidiMessage {
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
    NoteOn {
        channel: u8,
        pitch: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        pitch: u8,
    },
}

#[derive(Deserialize)]
pub(crate) struct InputPacket {
    pub source_unique_id: i32,
    #[serde(default)]
    pub host_time: u64,
    pub data_hex: String,
}

#[derive(Clone, Copy)]
pub(crate) struct BufferedMidiEvent {
    pub source_id: i32,
    pub host_time: u64,
    pub message: MidiMessage,
}

pub(crate) fn decode_midi_messages(
    data_hex: &str,
    running_status: &mut Option<u8>,
) -> Vec<MidiMessage> {
    let bytes = data_hex
        .as_bytes()
        .chunks_exact(2)
        .filter_map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect::<Vec<_>>();
    let mut events = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte >= 0xf8 {
            index += 1;
            continue;
        }
        if byte >= 0x80 {
            if byte >= 0xf0 {
                *running_status = None;
                // Skip SysEx and system-common data until the next status.
                index += 1;
                while index < bytes.len() && bytes[index] < 0x80 {
                    index += 1;
                }
                continue;
            }
            *running_status = Some(byte);
            index += 1;
            continue;
        }
        let Some(status) = *running_status else {
            index += 1;
            continue;
        };
        let kind = status & 0xf0;
        let data_len = if matches!(kind, 0xc0 | 0xd0) { 1 } else { 2 };
        if index + data_len > bytes.len() {
            break;
        }
        let data1 = bytes[index];
        let data2 = if data_len == 2 { bytes[index + 1] } else { 0 };
        if data1 < 128 && data2 < 128 {
            let channel = status & 0x0f;
            match kind {
                0xb0 => events.push(MidiMessage::ControlChange {
                    channel,
                    controller: data1,
                    value: data2,
                }),
                0x80 => events.push(MidiMessage::NoteOff {
                    channel,
                    pitch: data1,
                }),
                0x90 if data2 > 0 => events.push(MidiMessage::NoteOn {
                    channel,
                    pitch: data1,
                    velocity: data2,
                }),
                0x90 => events.push(MidiMessage::NoteOff {
                    channel,
                    pitch: data1,
                }),
                _ => {}
            }
        }
        index += data_len;
    }
    events
}

#[derive(Clone, Copy)]
struct OpenNote {
    start_sample: u64,
    start_elapsed_samples: u64,
    velocity: u8,
}

struct MidiTake {
    target_track_ids: Vec<u32>,
    start_host_time: u64,
    start_sample: u64,
    loop_range: Option<(u64, u64)>,
    open_notes: HashMap<(i32, u8, u8), VecDeque<OpenNote>>,
    notes: Vec<MidiNoteContract>,
}

impl MidiTake {
    fn new(
        target_track_ids: Vec<u32>,
        start_sample: u64,
        start_host_time: u64,
        loop_range: Option<(u64, u64)>,
    ) -> Self {
        Self {
            target_track_ids,
            start_host_time,
            start_sample,
            loop_range,
            open_notes: HashMap::new(),
            notes: Vec::new(),
        }
    }

    fn sample_at_elapsed(&self, elapsed: u64) -> u64 {
        let unwrapped = self.start_sample.saturating_add(elapsed);
        if let Some((loop_start, loop_end)) = self.loop_range {
            let span = loop_end.saturating_sub(loop_start);
            if span > 0 {
                let loop_origin = if self.start_sample >= loop_end {
                    loop_start
                } else {
                    self.start_sample
                };
                let position = loop_origin.saturating_add(elapsed);
                return if position >= loop_end {
                    loop_start.saturating_add(position.saturating_sub(loop_end) % span)
                } else {
                    position
                };
            }
        }
        unwrapped
    }

    fn consume(
        &mut self,
        source_id: i32,
        event_host_time: u64,
        message: MidiMessage,
        core: &HirariCore,
    ) {
        let elapsed_samples = core.midi_host_time_delta_samples(
            self.start_host_time,
            event_host_time,
            core.get_sample_rate(),
        );
        let sample = self.sample_at_elapsed(elapsed_samples);
        match message {
            MidiMessage::NoteOn {
                channel,
                pitch,
                velocity,
            } => {
                if velocity == 0 {
                    self.end_note(source_id, channel, pitch, elapsed_samples);
                    return;
                }
                let key = (source_id, channel, pitch);
                let queue = self.open_notes.entry(key).or_default();
                if queue.len() < 64 {
                    queue.push_back(OpenNote {
                        start_sample: sample,
                        start_elapsed_samples: elapsed_samples,
                        velocity,
                    });
                }
            }
            MidiMessage::NoteOff { channel, pitch } => {
                self.end_note(source_id, channel, pitch, elapsed_samples)
            }
            MidiMessage::ControlChange { .. } => {}
        }
    }

    fn end_note(&mut self, source_id: i32, channel: u8, pitch: u8, end_elapsed_samples: u64) {
        let key = (source_id, channel, pitch);
        let Some(note) = self.open_notes.get_mut(&key).and_then(VecDeque::pop_front) else {
            return;
        };
        if self.open_notes.get(&key).is_some_and(VecDeque::is_empty) {
            self.open_notes.remove(&key);
        }
        let length_samples = end_elapsed_samples
            .saturating_sub(note.start_elapsed_samples)
            .max(1);
        self.push_note(channel, pitch, note, length_samples);
    }

    fn finish(&mut self, stop_host_time: u64, core: &HirariCore) {
        let stop_elapsed_samples = core.midi_host_time_delta_samples(
            self.start_host_time,
            stop_host_time,
            core.get_sample_rate(),
        );
        let open = std::mem::take(&mut self.open_notes);
        for ((_, channel, pitch), queue) in open {
            for note in queue {
                let length_samples = stop_elapsed_samples
                    .saturating_sub(note.start_elapsed_samples)
                    .max(1);
                self.push_note(channel, pitch, note, length_samples);
            }
        }
    }

    fn push_note(&mut self, channel: u8, pitch: u8, note: OpenNote, length_samples: u64) {
        let length = length_samples.max(1);
        if let Some((loop_start, loop_end)) = self.loop_range {
            let span = loop_end.saturating_sub(loop_start);
            if span > 0 {
                let mut cursor = note.start_sample;
                let mut remaining = length;
                while remaining > 0 {
                    if cursor >= loop_end {
                        cursor = loop_start.saturating_add(cursor.saturating_sub(loop_end) % span);
                    }
                    let until_loop_end = loop_end.saturating_sub(cursor);
                    if until_loop_end == 0 {
                        cursor = loop_start;
                        continue;
                    }
                    let segment_length = remaining.min(until_loop_end);
                    self.push_note_segment(
                        channel,
                        pitch,
                        OpenNote {
                            start_sample: cursor,
                            ..note
                        },
                        segment_length,
                    );
                    remaining -= segment_length;
                    cursor = cursor.saturating_add(segment_length);
                    if cursor >= loop_end {
                        cursor = loop_start;
                    }
                }
                return;
            }
        }
        self.push_note_segment(channel, pitch, note, length);
    }

    fn push_note_segment(&mut self, channel: u8, pitch: u8, note: OpenNote, length_samples: u64) {
        for track_id in &self.target_track_ids {
            self.notes.push(MidiNoteContract {
                region_id: 0,
                track_id: *track_id,
                pitch,
                midi_channel: channel,
                articulation: 0,
                velocity: note.velocity.clamp(1, 127),
                start_sample: note.start_sample,
                length_samples: length_samples.max(1),
                lyric: String::new(),
                phoneme: String::new(),
                pitch_curve_cents: Vec::new(),
                vibrato_depth_cents: 0,
                vibrato_rate_millihz: 5_000,
                portamento_samples: 0,
                probability: 100,
                repeat_count: 1,
            });
        }
    }
}

/// Insert the last ~30 s of buffered Note On/Off events on `track_id` as a
/// single undoable MIDI region. Returns the number of notes inserted.
pub(crate) fn insert_retrospective_notes(
    track_id: u32,
    buffered_events: &RefCell<VecDeque<BufferedMidiEvent>>,
    core: &HirariCore,
    is_midi_track: bool,
) -> Result<usize, String> {
    if !is_midi_track {
        return Err(
            "RETROSPECTIVE MIDI INSERT FAILED: SELECT A MIDI OR INSTRUMENT TRACK".to_owned(),
        );
    }
    let core_now = core.midi_host_time_now();
    let sample_rate = core.get_sample_rate();
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err("RETROSPECTIVE MIDI INSERT FAILED: INVALID SAMPLE RATE".to_owned());
    }
    {
        let mut buffer = buffered_events.borrow_mut();
        while buffer.front().is_some_and(|event| {
            core.midi_host_time_delta_samples(event.host_time, core_now, sample_rate)
                > sample_rate as u64 * RETROSPECTIVE_MIDI_SECONDS
        }) {
            buffer.pop_front();
        }
    }
    let mut events = buffered_events.borrow().iter().copied().collect::<Vec<_>>();
    events.sort_by_key(|event| event.host_time);
    let Some(first_event) = events.first() else {
        return Err("RETROSPECTIVE MIDI: NO RECENT NOTE INPUT".to_owned());
    };
    let elapsed = core.midi_host_time_delta_samples(first_event.host_time, core_now, sample_rate);
    let current_sample = core.get_playhead();
    let loop_range = if core.is_loop_enabled() {
        let start = core.cycle_start_sample();
        let end = core.cycle_end_sample();
        (end > start).then_some((start, end))
    } else {
        None
    };
    let start_sample = if let Some((loop_start, loop_end)) = loop_range {
        let span = loop_end - loop_start;
        let current_offset = current_sample.saturating_sub(loop_start) % span;
        let elapsed_offset = elapsed % span;
        let start_offset = if current_offset >= elapsed_offset {
            current_offset - elapsed_offset
        } else {
            span - (elapsed_offset - current_offset)
        };
        loop_start + start_offset
    } else {
        current_sample.saturating_sub(elapsed)
    };
    let mut take = MidiTake::new(
        vec![track_id],
        start_sample,
        first_event.host_time,
        loop_range,
    );
    for event in events {
        take.consume(event.source_id, event.host_time, event.message, core);
    }
    take.finish(core_now, core);
    let recorded_count = take.notes.len();
    if recorded_count == 0 {
        return Err("RETROSPECTIVE MIDI: NO COMPLETE NOTES IN BUFFER".to_owned());
    }
    let mut notes =
        serde_json::from_str::<Vec<MidiNoteContract>>(&core.midi_notes_json()).unwrap_or_default();
    notes.extend(take.notes);
    core.begin_undo_transaction("Insert Retrospective MIDI");
    if !core.replace_midi_note_contracts(notes, true) {
        let _ = core.abort_undo_transaction();
        return Err("RETROSPECTIVE MIDI INSERT FAILED: CORE REJECTED NOTES".to_owned());
    }
    if !core.end_undo_transaction() {
        let _ = core.abort_undo_transaction();
        return Err("RETROSPECTIVE MIDI INSERT FAILED: UNDO COMMIT".to_owned());
    }
    Ok(recorded_count)
}
