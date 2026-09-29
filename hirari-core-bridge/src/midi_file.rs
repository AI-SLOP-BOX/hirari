//! Standard MIDI File (SMF) import/export for the canonical scheduled-note
//! model. File operations stay on the control plane; audio callbacks never
//! touch the filesystem or allocate MIDI event buffers.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

use crate::project_contracts::MidiNoteContract;
use crate::project_contracts::{TempoEventContract, TimeSignatureEventContract};

const PPQ: u32 = 480;
const MAX_NOTES: usize = 1_000_000;
const MAX_TEMPO_POINTS: usize = 1_000_000;
const MAX_IMPORT_BYTES: usize = 64 * 1024 * 1024;
const MAX_IMPORT_TRACKS: usize = 256;
const MAX_IMPORT_EVENTS: usize = 2_000_000;
const MAX_IMPORT_MAP_EVENTS: usize = 2_048;

/// MIDI note positions expressed in musical beats after conversion through
/// the project's tempo map. Standard MIDI stores event ticks, not samples.
#[derive(Debug, Clone)]
pub struct MidiExportNote {
    pub note: MidiNoteContract,
    pub start_beat: f64,
    pub end_beat: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedMidiNote {
    pub start_tick: u64,
    pub end_tick: u64,
    pub pitch: u8,
    pub velocity: u8,
    pub midi_channel: u8,
    pub lyric: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedMidiTrack {
    pub name: String,
    pub notes: Vec<ImportedMidiNote>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StandardMidiFile {
    pub ppq: u16,
    pub tracks: Vec<ImportedMidiTrack>,
    pub tempo_map: Vec<TempoEventContract>,
    pub has_explicit_tempo_map: bool,
    pub time_signature_map: Vec<TimeSignatureEventContract>,
}

struct SmfReader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> SmfReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self
            .cursor
            .checked_add(count)
            .ok_or_else(|| "MIDI chunk offset overflow".to_owned())?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or_else(|| "truncated MIDI file".to_owned())?;
        self.cursor = end;
        Ok(value)
    }

    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn vlq(&mut self) -> Result<u32, String> {
        let mut value = 0u32;
        for index in 0..4 {
            let byte = self.byte()?;
            value = (value << 7) | u32::from(byte & 0x7f);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            if index == 3 {
                return Err("invalid MIDI variable-length quantity".into());
            }
        }
        Err("invalid MIDI variable-length quantity".into())
    }
}

/// Reads format 0/1 Standard MIDI files using PPQ timing. Unsupported SMPTE
/// timing and format-2 pattern files are rejected instead of being misread.
pub fn read_standard_midi(path: &Path) -> Result<StandardMidiFile, String> {
    let file = File::open(path).map_err(|error| format!("could not read MIDI file: {error}"))?;
    let file_size = file
        .metadata()
        .map_err(|error| format!("could not inspect MIDI file: {error}"))?
        .len();
    if file_size > MAX_IMPORT_BYTES as u64 {
        return Err("MIDI file exceeds the 64 MiB import limit".into());
    }
    let mut bytes = Vec::with_capacity(file_size as usize);
    file.take((MAX_IMPORT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read MIDI file: {error}"))?;
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err("MIDI file exceeds the 64 MiB import limit".into());
    }
    parse_standard_midi(&bytes)
}

pub fn parse_standard_midi(bytes: &[u8]) -> Result<StandardMidiFile, String> {
    if bytes.is_empty() || bytes.len() > MAX_IMPORT_BYTES {
        return Err("invalid MIDI file size".into());
    }
    let mut reader = SmfReader::new(bytes);
    if reader.take(4)? != b"MThd" {
        return Err("not a Standard MIDI file".into());
    }
    let header_len = reader.u32()? as usize;
    if header_len < 6 {
        return Err("invalid MIDI header length".into());
    }
    let mut header = SmfReader::new(reader.take(header_len)?);
    let format = header.u16()?;
    let track_count = usize::from(header.u16()?);
    let division = header.u16()?;
    if format > 1 {
        return Err("MIDI format 2 is not supported".into());
    }
    if track_count == 0 || track_count > MAX_IMPORT_TRACKS || (format == 0 && track_count != 1) {
        return Err("invalid MIDI track count".into());
    }
    if division == 0 || division & 0x8000 != 0 {
        return Err("SMPTE-timed MIDI files are not supported".into());
    }

    let mut tracks = Vec::with_capacity(track_count);
    let mut tempo_events = Vec::<(u64, u32, usize)>::new();
    let mut time_signature_events = Vec::<(u64, u8, u8, usize)>::new();
    let mut total_events = 0usize;
    for track_index in 0..track_count {
        let chunk_id = reader.take(4)?;
        let chunk_len = reader.u32()? as usize;
        let chunk = reader.take(chunk_len)?;
        if chunk_id != b"MTrk" {
            return Err(format!("expected MIDI track chunk {track_index}"));
        }
        let (track, events_seen) = parse_midi_track(
            chunk,
            track_index,
            &mut tempo_events,
            &mut time_signature_events,
        )?;
        total_events = total_events.saturating_add(events_seen);
        if total_events > MAX_IMPORT_EVENTS {
            return Err("MIDI file has too many events".into());
        }
        tracks.push(track);
    }

    let has_explicit_tempo_map = !tempo_events.is_empty();
    tempo_events.sort_by_key(|(tick, _micros, order)| (*tick, *order));
    let mut tempo_map: Vec<TempoEventContract> =
        Vec::with_capacity(tempo_events.len().saturating_add(1));
    for (tick, micros_per_quarter, _) in tempo_events {
        if micros_per_quarter == 0 {
            return Err("MIDI tempo event has zero duration".into());
        }
        let beat = tick as f64 / f64::from(division);
        let bpm = 60_000_000.0 / f64::from(micros_per_quarter);
        if !(20.0..=300.0).contains(&bpm) {
            return Err("MIDI tempo is outside Hirari's supported range (20–300 BPM)".into());
        }
        if let Some(existing) = tempo_map.last_mut() {
            if (existing.beat - beat).abs() < f64::EPSILON {
                existing.bpm = bpm;
                existing.ramp = false;
                continue;
            }
        }
        tempo_map.push(TempoEventContract {
            beat,
            bpm,
            ramp: false,
        });
    }
    if tempo_map.first().map_or(true, |event| event.beat > 0.0) {
        tempo_map.insert(
            0,
            TempoEventContract {
                beat: 0.0,
                bpm: 120.0,
                ramp: false,
            },
        );
    }
    time_signature_events.sort_by_key(|(tick, _, _, order)| (*tick, *order));
    let has_explicit_time_signature_map = !time_signature_events.is_empty();
    let mut time_signature_map: Vec<TimeSignatureEventContract> = Vec::new();
    for (tick, numerator, denominator, _) in time_signature_events {
        let event = TimeSignatureEventContract {
            beat: tick as f64 / f64::from(division),
            numerator,
            denominator,
        };
        event.validate().map_err(|error| error.to_string())?;
        if let Some(existing) = time_signature_map.last_mut() {
            if (existing.beat - event.beat).abs() < f64::EPSILON {
                *existing = event;
                continue;
            }
        }
        time_signature_map.push(event);
    }
    if has_explicit_time_signature_map
        && time_signature_map
            .first()
            .map_or(true, |event| event.beat > 0.0)
    {
        time_signature_map.insert(
            0,
            TimeSignatureEventContract {
                beat: 0.0,
                numerator: 4,
                denominator: 4,
            },
        );
    }
    Ok(StandardMidiFile {
        ppq: division,
        tracks,
        tempo_map,
        has_explicit_tempo_map,
        time_signature_map,
    })
}

fn parse_midi_track(
    bytes: &[u8],
    track_index: usize,
    tempo_events: &mut Vec<(u64, u32, usize)>,
    time_signature_events: &mut Vec<(u64, u8, u8, usize)>,
) -> Result<(ImportedMidiTrack, usize), String> {
    let mut reader = SmfReader::new(bytes);
    let mut tick = 0u64;
    let mut last_tick = 0u64;
    let mut running_status = None;
    let mut name = String::new();
    let mut pending = BTreeMap::<(u8, u8), VecDeque<(u64, usize)>>::new();
    let mut notes = Vec::new();
    let mut note_starts = Vec::<(u64, usize)>::new();
    let mut lyrics = BTreeMap::<u64, Vec<String>>::new();
    let mut event_count = 0usize;
    let mut open_note_count = 0usize;

    while reader.cursor < bytes.len() {
        event_count += 1;
        if event_count > MAX_IMPORT_EVENTS {
            return Err("MIDI track has too many events".into());
        }
        tick = tick
            .checked_add(u64::from(reader.vlq()?))
            .ok_or_else(|| "MIDI event position overflow".to_owned())?;
        last_tick = tick;
        let first = reader.byte()?;
        let status = if first & 0x80 != 0 {
            first
        } else {
            reader.cursor -= 1;
            running_status.ok_or_else(|| "MIDI data byte has no running status".to_owned())?
        };

        if status == 0xff {
            running_status = None;
            let meta_type = reader.byte()?;
            let length = reader.vlq()? as usize;
            let data = reader.take(length)?;
            match meta_type {
                0x03 => name = String::from_utf8_lossy(data).trim().to_owned(),
                0x05 => {
                    let lyric = String::from_utf8_lossy(data).trim().to_owned();
                    if !lyric.is_empty() && lyric.len() <= 1024 {
                        lyrics.entry(tick).or_default().push(lyric);
                    }
                }
                0x51 if data.len() == 3 => {
                    if tempo_events.len() >= MAX_IMPORT_MAP_EVENTS {
                        return Err("MIDI tempo map has too many events".into());
                    }
                    let micros =
                        (u32::from(data[0]) << 16) | (u32::from(data[1]) << 8) | u32::from(data[2]);
                    tempo_events.push((tick, micros, track_index));
                }
                0x58 if data.len() >= 2 => {
                    if time_signature_events.len() >= MAX_IMPORT_MAP_EVENTS {
                        return Err("MIDI meter map has too many events".into());
                    }
                    let numerator = data[0];
                    let denominator = 1u8.checked_shl(u32::from(data[1])).ok_or_else(|| {
                        "MIDI time signature denominator is unsupported".to_owned()
                    })?;
                    let event = TimeSignatureEventContract {
                        beat: tick as f64,
                        numerator,
                        denominator,
                    };
                    event.validate().map_err(|error| error.to_string())?;
                    time_signature_events.push((tick, numerator, denominator, track_index));
                }
                0x2f => break,
                _ => {}
            }
            continue;
        }
        if status == 0xf0 || status == 0xf7 {
            running_status = None;
            let length = reader.vlq()? as usize;
            let _ = reader.take(length)?;
            continue;
        }

        if !(0x80..=0xef).contains(&status) {
            return Err("unsupported system event in MIDI track".into());
        }
        running_status = Some(status);
        let event_type = status & 0xf0;
        let channel = status & 0x0f;
        let data_len = if matches!(event_type, 0xc0 | 0xd0) {
            1
        } else {
            2
        };
        let data1 = reader.byte()?;
        let data2 = if data_len == 2 { reader.byte()? } else { 0 };
        if data1 >= 0x80 || data2 >= 0x80 {
            return Err("invalid MIDI channel event data".into());
        }
        if event_type == 0x90 && data2 != 0 {
            let key = (channel, data1);
            if open_note_count >= MAX_NOTES {
                return Err("MIDI track has too many open notes".into());
            }
            let index = notes.len();
            notes.push(ImportedMidiNote {
                start_tick: tick,
                end_tick: tick,
                pitch: data1,
                velocity: data2,
                midi_channel: channel,
                lyric: String::new(),
            });
            pending.entry(key).or_default().push_back((tick, index));
            open_note_count += 1;
            note_starts.push((tick, index));
        } else if event_type == 0x80 || (event_type == 0x90 && data2 == 0) {
            if let Some((start_tick, index)) = pending
                .get_mut(&(channel, data1))
                .and_then(VecDeque::pop_front)
            {
                open_note_count = open_note_count.saturating_sub(1);
                notes[index].end_tick = tick.max(start_tick.saturating_add(1));
            }
        }
    }

    for queue in pending.values_mut() {
        while let Some((start_tick, index)) = queue.pop_front() {
            notes[index].end_tick = last_tick.max(start_tick.saturating_add(1));
        }
    }
    let mut next_note = 0usize;
    for (lyric_tick, lyric_texts) in lyrics {
        while note_starts
            .get(next_note)
            .is_some_and(|(start_tick, _)| *start_tick < lyric_tick)
        {
            next_note += 1;
        }
        for lyric in lyric_texts {
            let Some((_, note_index)) = note_starts.get(next_note) else {
                break;
            };
            notes[*note_index].lyric = lyric;
            next_note += 1;
        }
    }
    notes.sort_by_key(|note| (note.start_tick, note.midi_channel, note.pitch));
    Ok((ImportedMidiTrack { name, notes }, event_count))
}

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

fn beat_to_tick(beat: f64) -> Option<u32> {
    let ticks = beat * f64::from(PPQ);
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

#[derive(Debug, Clone, Copy)]
enum ConductorEvent {
    Tempo(f64),
    TimeSignature(u8, u8),
}

fn tempo_track(
    tempo_map: &[TempoEventContract],
    time_signature_map: &[TimeSignatureEventContract],
) -> Result<Vec<u8>, String> {
    if tempo_map.is_empty() || tempo_map.len() > MAX_TEMPO_POINTS {
        return Err("invalid MIDI tempo map".into());
    }
    let mut ordered = tempo_map.to_vec();
    ordered.sort_by(|left, right| left.beat.total_cmp(&right.beat));
    if ordered.iter().any(|event| {
        !event.beat.is_finite()
            || event.beat < 0.0
            || !event.bpm.is_finite()
            || !(20.0..=300.0).contains(&event.bpm)
    }) {
        return Err("invalid MIDI tempo event".into());
    }

    // SMF tempo events are step changes. Approximate the engine's continuous
    // ramps with 1/16-beat steps so exported note times remain aligned during
    // playback in readers that do not support Hirari's ramp metadata.
    let mut points = BTreeMap::<u32, f64>::new();
    for (index, event) in ordered.iter().enumerate() {
        let start_tick = beat_to_tick(event.beat)
            .ok_or_else(|| "MIDI tempo position is out of range".to_owned())?;
        let Some(next) = ordered.get(index + 1) else {
            points.insert(start_tick, event.bpm);
            continue;
        };
        let end_tick = beat_to_tick(next.beat)
            .ok_or_else(|| "MIDI tempo position is out of range".to_owned())?;
        if event.ramp && end_tick > start_tick {
            let span = end_tick - start_tick;
            let steps = span.div_ceil((PPQ / 16).max(1));
            if points.len().saturating_add(steps as usize) > MAX_TEMPO_POINTS {
                return Err("MIDI tempo ramp expands beyond the event limit".into());
            }
            for step in 0..steps {
                let progress = f64::from(step) / f64::from(steps);
                let tick =
                    start_tick + ((u64::from(span) * u64::from(step)) / u64::from(steps)) as u32;
                points.insert(tick, event.bpm + (next.bpm - event.bpm) * progress);
            }
        } else {
            points.insert(start_tick, event.bpm);
        }
    }

    if time_signature_map.len() > MAX_TEMPO_POINTS {
        return Err("MIDI meter map has too many events".into());
    }
    let mut conductor_events = BTreeMap::<u32, Vec<ConductorEvent>>::new();
    for (tick, bpm) in points {
        conductor_events
            .entry(tick)
            .or_default()
            .push(ConductorEvent::Tempo(bpm));
    }
    for event in time_signature_map {
        event.validate().map_err(|error| error.to_string())?;
        let denominator_power = event.denominator.trailing_zeros();
        if 1u8.checked_shl(denominator_power) != Some(event.denominator) {
            return Err("MIDI meter denominator must be a power of two".into());
        }
        let tick = beat_to_tick(event.beat)
            .ok_or_else(|| "MIDI meter position is out of range".to_owned())?;
        conductor_events
            .entry(tick)
            .or_default()
            .push(ConductorEvent::TimeSignature(
                event.numerator,
                event.denominator,
            ));
    }

    let mut events = Vec::with_capacity(
        conductor_events
            .values()
            .map(Vec::len)
            .sum::<usize>()
            .saturating_mul(7)
            .saturating_add(4),
    );
    let mut cursor = 0u32;
    for (tick, mut at_tick) in conductor_events {
        at_tick.sort_by_key(|event| matches!(event, ConductorEvent::TimeSignature(..)) as u8);
        for event in at_tick {
            push_vlq(&mut events, tick.saturating_sub(cursor));
            match event {
                ConductorEvent::Tempo(bpm) => {
                    let micros = (60_000_000.0 / bpm).round().clamp(1.0, 16_777_215.0) as u32;
                    events.extend_from_slice(&[
                        0xff,
                        0x51,
                        3,
                        ((micros >> 16) & 0xff) as u8,
                        ((micros >> 8) & 0xff) as u8,
                        (micros & 0xff) as u8,
                    ]);
                }
                ConductorEvent::TimeSignature(numerator, denominator) => {
                    events.extend_from_slice(&[
                        0xff,
                        0x58,
                        4,
                        numerator,
                        denominator.trailing_zeros() as u8,
                        24,
                        8,
                    ]);
                }
            }
            cursor = tick;
        }
    }
    events.extend_from_slice(&[0, 0xff, 0x2f, 0]);
    Ok(track_chunk(events))
}

/// Writes a format-1 SMF with a tempo track and one MIDI track per Hirari track.
/// Returns the number of exported notes. Existing files are replaced only
/// after the complete temporary file has been flushed and synced.
pub fn write_standard_midi(
    path: &Path,
    notes: &[MidiNoteContract],
    sample_rate: f64,
    bpm: f64,
) -> Result<usize, String> {
    if !sample_rate.is_finite() || !(8_000.0..=384_000.0).contains(&sample_rate) {
        return Err("invalid MIDI export sample rate".into());
    }
    if !bpm.is_finite() || !(1.0..=999.0).contains(&bpm) {
        return Err("invalid MIDI export tempo".into());
    }
    let beats_per_sample = bpm / (60.0 * sample_rate);
    let mut positioned = Vec::with_capacity(notes.len());
    for note in notes {
        let end_sample = note
            .start_sample
            .checked_add(note.length_samples)
            .ok_or_else(|| "MIDI note end overflows".to_owned())?;
        positioned.push(MidiExportNote {
            note: note.clone(),
            start_beat: note.start_sample as f64 * beats_per_sample,
            end_beat: end_sample as f64 * beats_per_sample,
        });
    }
    write_standard_midi_with_tempo_map(
        path,
        &positioned,
        &[TempoEventContract {
            beat: 0.0,
            bpm,
            ramp: false,
        }],
    )
}

/// Exports notes already converted to beats by the project's tempo map and
/// writes all tempo changes to the SMF conductor track.
pub fn write_standard_midi_with_tempo_map(
    path: &Path,
    notes: &[MidiExportNote],
    tempo_map: &[TempoEventContract],
) -> Result<usize, String> {
    write_standard_midi_with_maps(path, notes, tempo_map, &[])
}

/// Exports notes, tempo changes, and time signatures in one SMF transaction.
pub fn write_standard_midi_with_maps(
    path: &Path,
    notes: &[MidiExportNote],
    tempo_map: &[TempoEventContract],
    time_signature_map: &[TimeSignatureEventContract],
) -> Result<usize, String> {
    if path.as_os_str().is_empty() || notes.len() > MAX_NOTES {
        return Err("invalid MIDI export path or note count".into());
    }
    let mut grouped = BTreeMap::<u32, Vec<MidiEvent>>::new();
    for positioned in notes {
        let note = &positioned.note;
        note.validate().map_err(|error| error.to_string())?;
        if !positioned.start_beat.is_finite()
            || !positioned.end_beat.is_finite()
            || positioned.start_beat < 0.0
            || positioned.end_beat < positioned.start_beat
        {
            return Err("invalid MIDI note beat position".into());
        }
        let start = beat_to_tick(positioned.start_beat)
            .ok_or_else(|| "MIDI note start is out of range".to_owned())?;
        let end = beat_to_tick(positioned.end_beat)
            .ok_or_else(|| "MIDI note end is out of range".to_owned())?
            .max(start.saturating_add(1));
        let channel = note.midi_channel;
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
    file_data.extend_from_slice(&tempo_track(tempo_map, time_signature_map)?);

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
    let temporary = parent.join(format!(".{name}.hirari-midi-{}.tmp", std::process::id()));
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
            region_id: 0,
            midi_channel: 0,
            articulation: 0,
            track_id,
            pitch: 60,
            velocity: 100,
            start_sample,
            length_samples: 24_000,
            lyric: String::new(),
            phoneme: String::new(),
            pitch_curve_cents: Vec::new(),
            vibrato_depth_cents: 0,
            vibrato_rate_millihz: 5_000,
            portamento_samples: 0,
            probability: 100,
            repeat_count: 1,
        }
    }

    #[test]
    fn writes_format_one_midi_with_tempo_and_track_chunks() {
        let path = std::env::temp_dir().join(format!("hirari-smf-{}.mid", std::process::id()));
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
            std::env::temp_dir().join(format!("hirari-smf-invalid-{}.mid", std::process::id()));
        let mut invalid = note(1, 0);
        invalid.velocity = 0;
        assert!(write_standard_midi(&path, &[invalid], 48_000.0, 120.0).is_err());
        assert!(!path.exists());
    }
}
