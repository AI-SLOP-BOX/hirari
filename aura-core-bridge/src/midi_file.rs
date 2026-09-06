//! Standard MIDI File (SMF) export for the canonical scheduled-note model.
//! The writer is deliberately control-plane only: no audio callback touches
//! the filesystem or allocates MIDI event buffers.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use crate::project_contracts::MidiNoteContract;

const PPQ: u32 = 480;
const MAX_NOTES: usize = 1_000_000;

#[derive(Debug, Clone)]
enum EventKind {
    NoteOff,
    Lyric(String),
    NoteOn,
}

#[derive(Debug, Clone)]
struct MidiEvent {
    tick: u32,
    kind: EventKind,
    channel: u8,
    pitch: u8,
    velocity: u8,
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_vlq(out: &mut Vec<u8>, mut value: u32) {
    let mut bytes = [0u8; 4];
    let mut index = 3;
    bytes[index] = (value & 0x7f) as u8;
    while {
        value >>= 7;
        value != 0
    } {
        index -= 1;
        bytes[index] = ((value & 0x7f) as u8) | 0x80;
    }
    out.extend_from_slice(&bytes[index..]);
}

fn sample_to_tick(sample: u64, sample_rate: f64, bpm: f64) -> Option<u32> {
    let ticks = sample as f64 * bpm * f64::from(PPQ) / (60.0 * sample_rate);
    if !ticks.is_finite() || ticks < 0.0 || ticks > f64::from(u32::MAX) {
        return None;
    }
    Some(ticks.round() as u32)
}

fn track_chunk(events: Vec<u8>) -> Vec<u8> {
    let mut chunk = Vec::with_capacity(8 + events.len());
    chunk.extend_from_slice(b"MTrk");
    push_u32(&mut chunk, events.len() as u32);
    chunk.extend_from_slice(&events);
    chunk
}

fn tempo_track(bpm: f64) -> Vec<u8> {
    let micros = (60_000_000.0 / bpm).round().clamp(1.0, f64::from(u32::MAX)) as u32;
    let mut events = Vec::with_capacity(16);
    events.extend_from_slice(&[0, 0xff, 0x51, 3]);
    events.extend_from_slice(&micros.to_be_bytes()[1..]);
    events.extend_from_slice(&[0, 0xff, 0x2f, 0]);
    track_chunk(events)
}

/// Writes a format-1 SMF with a tempo track and one MIDI track per Aura track.
/// Returns the number of exported notes. Existing files are replaced only
/// after the complete temporary file has been flushed and synced.
pub fn write_standard_midi(
    path: &Path,
    notes: &[MidiNoteContract],
    sample_rate: f64,
    bpm: f64,
) -> Result<usize, String> {
    if path.as_os_str().is_empty() || notes.len() > MAX_NOTES {
        return Err("invalid MIDI export path or note count".into());
    }
    if !sample_rate.is_finite() || !(8_000.0..=384_000.0).contains(&sample_rate) {
        return Err("invalid MIDI export sample rate".into());
    }
    if !bpm.is_finite() || !(1.0..=999.0).contains(&bpm) {
        return Err("invalid MIDI export tempo".into());
    }
    let mut grouped = BTreeMap::<u32, Vec<MidiEvent>>::new();
    for note in notes {
        note.validate().map_err(|error| error.to_string())?;
        let start = sample_to_tick(note.start_sample, sample_rate, bpm)
            .ok_or_else(|| "MIDI note start is out of range".to_owned())?;
        let end_sample = note
            .start_sample
            .checked_add(note.length_samples)
            .ok_or_else(|| "MIDI note end overflows".to_owned())?;
        let end = sample_to_tick(end_sample, sample_rate, bpm)
            .ok_or_else(|| "MIDI note end is out of range".to_owned())?;
        let end = end.max(start.saturating_add(1));
        let channel = (note.track_id % 16) as u8;
        let entry = grouped.entry(note.track_id).or_default();
        entry.push(MidiEvent {
            tick: start,
            kind: EventKind::NoteOn,
            channel,
            pitch: note.pitch,
            velocity: note.velocity,
        });
        if !note.lyric.is_empty() {
            entry.push(MidiEvent {
                tick: start,
                kind: EventKind::Lyric(note.lyric.clone()),
                channel,
                pitch: 0,
                velocity: 0,
            });
        }
        entry.push(MidiEvent {
            tick: end,
            kind: EventKind::NoteOff,
            channel,
            pitch: note.pitch,
            velocity: 0,
        });
    }

    let track_count = grouped.len().saturating_add(1);
    if track_count > usize::from(u16::MAX) {
        return Err("too many MIDI tracks".into());
    }
    let mut file_data = Vec::new();
    file_data.extend_from_slice(b"MThd");
    push_u32(&mut file_data, 6);
    push_u16(&mut file_data, 1);
    push_u16(&mut file_data, track_count as u16);
    push_u16(&mut file_data, PPQ as u16);
    file_data.extend_from_slice(&tempo_track(bpm));

    for (_track_id, mut events) in grouped {
        // Note-offs precede note-ons at the same tick to avoid stuck/retriggered
        // voices in strict external MIDI readers.
        events.sort_by_key(|event| {
            let order = match &event.kind {
                EventKind::NoteOff => 0u8,
                EventKind::Lyric(_) => 1,
                EventKind::NoteOn => 2,
            };
            (event.tick, order)
        });
        let mut encoded = Vec::new();
        let mut cursor = 0u32;
        for event in events {
            push_vlq(&mut encoded, event.tick.saturating_sub(cursor));
            match event.kind {
                EventKind::NoteOff => {
                    encoded.push(0x80 | event.channel);
                    encoded.push(event.pitch);
                    encoded.push(0);
                }
                EventKind::NoteOn => {
                    encoded.push(0x90 | event.channel);
                    encoded.push(event.pitch);
                    encoded.push(event.velocity);
                }
                EventKind::Lyric(text) => {
                    encoded.extend_from_slice(&[0xff, 0x05]);
                    push_vlq(&mut encoded, text.len() as u32);
                    encoded.extend_from_slice(text.as_bytes());
                }
            }
            cursor = event.tick;
        }
        encoded.extend_from_slice(&[0, 0xff, 0x2f, 0]);
        file_data.extend_from_slice(&track_chunk(encoded));
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "MIDI export filename is invalid".to_owned())?;
    let temporary = parent.join(format!(".{name}.aura-midi-{}.tmp", std::process::id()));
    let result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&file_data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(notes.len())
}

#[cfg(test)]
mod tests {
    use super::write_standard_midi;
    use crate::project_contracts::MidiNoteContract;

    fn note(track_id: u32, start_sample: u64) -> MidiNoteContract {
        MidiNoteContract {
            track_id,
            pitch: 60,
            velocity: 100,
            start_sample,
            length_samples: 24_000,
            lyric: String::new(),
            phoneme: String::new(),
            pitch_curve_cents: Vec::new(),
            vibrato_depth_cents: 0,
            portamento_samples: 0,
            probability: 100,
            repeat_count: 1,
        }
    }

    #[test]
    fn writes_format_one_midi_with_tempo_and_track_chunks() {
        let path = std::env::temp_dir().join(format!("aura-smf-{}.mid", std::process::id()));
        let mut first = note(1, 0);
        first.velocity = 37;
        first.lyric = "la".into();
        let count = write_standard_midi(&path, &[first, note(2, 48_000)], 48_000.0, 120.0)
            .expect("SMF export must succeed");
        let data = std::fs::read(&path).expect("SMF must be readable");
        assert_eq!(count, 2);
        assert_eq!(&data[..4], b"MThd");
        assert_eq!(&data[10..12], &[0, 3]);
        assert!(data.windows(4).filter(|chunk| *chunk == b"MTrk").count() == 3);
        assert!(data.windows(3).any(|chunk| chunk == [0x91, 60, 37]));
        assert!(data
            .windows(5)
            .any(|chunk| chunk == [0xff, 0x05, 2, b'l', b'a']));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_invalid_note_without_publishing_a_file() {
        let path =
            std::env::temp_dir().join(format!("aura-smf-invalid-{}.mid", std::process::id()));
        let mut invalid = note(1, 0);
        invalid.velocity = 0;
        assert!(write_standard_midi(&path, &[invalid], 48_000.0, 120.0).is_err());
        assert!(!path.exists());
    }
}
