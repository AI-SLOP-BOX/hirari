//! Packed-note vector codec: nine-field layout decode/encode FFI, split out
//! of midi_note_edit.rs.
use super::*;

pub(crate) const PACKED_NOTE_FIELDS: usize = 9;
pub(crate) const MAX_PACKED_NOTES: usize = 100_000;

pub(crate) fn decode_packed_note(fields: &[u64]) -> Option<ScheduledMidiNoteFfi> {
    let [track, pitch, velocity, midi_channel, start, length, probability, region, articulation] =
        [fields[0], fields[1], fields[2], fields[3], fields[4], fields[5], fields[6], fields[7], fields[8]];
    if track > u64::from(u32::MAX)
        || pitch > 127
        || velocity == 0
        || velocity > 127
        || midi_channel > 15
        || length == 0
        || probability > 100
        || region > u64::from(u32::MAX)
        || articulation > 127
        || start.checked_add(length).is_none()
    {
        return None;
    }
    Some(ScheduledMidiNoteFfi {
        track_id: track as u32,
        pitch: pitch as u8,
        velocity: velocity as u8,
        midi_channel: midi_channel as u8,
        articulation_id: articulation as u8,
        start_sample: start,
        length_samples: length,
        probability: probability as u8,
        region_id: region as u32,
    })
}

pub(crate) fn packed_notes(packed: &[u64]) -> Option<impl Iterator<Item = &[u64]>> {
    if packed.len() % PACKED_NOTE_FIELDS != 0 || packed.len() / PACKED_NOTE_FIELDS > MAX_PACKED_NOTES
    {
        return None;
    }
    Some(packed.chunks_exact(PACKED_NOTE_FIELDS))
}

/// Decodes the UI bridge's nine-field note vector into the note-table layout.
/// Every note is range-checked (ids, MIDI bytes, non-zero length, probability,
/// no start/length overflow) before any byte is written, so a malformed vector
/// leaves the caller's buffer untouched. Returns the note count, or
/// `usize::MAX` when the vector is malformed or a note is out of range; a
/// smaller `capacity` reports the count without writing.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_notes_unpack(
    packed: *const u64,
    len: usize,
    out: *mut ScheduledMidiNoteFfi,
    capacity: usize,
) -> usize {
    if len == 0 {
        return 0;
    }
    if packed.is_null() {
        return usize::MAX;
    }
    let packed = unsafe { std::slice::from_raw_parts(packed, len) };
    let Some(chunks) = packed_notes(packed) else {
        return usize::MAX;
    };
    let count = len / PACKED_NOTE_FIELDS;
    if count == 0 || out.is_null() || capacity < count {
        // Validate without writing so the count-then-copy protocol never
        // reports a size for a vector that would be rejected.
        return if chunks.map(decode_packed_note).all(|note| note.is_some()) {
            count
        } else {
            usize::MAX
        };
    }
    let notes = unsafe { std::slice::from_raw_parts_mut(out, count) };
    for (slot, fields) in notes.iter_mut().zip(chunks) {
        let Some(note) = decode_packed_note(fields) else {
            return usize::MAX;
        };
        *slot = note;
    }
    count
}

/// Packs the note-table layout into the nine-field vector the UI bridge reads.
/// Returns the required element count (`note_count * 9`) and copies only when
/// the capacity covers it, so the caller can size its vector in one call.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_notes_pack(
    notes: *const ScheduledMidiNoteFfi,
    note_count: usize,
    out: *mut u64,
    capacity: usize,
) -> usize {
    if note_count == 0 {
        return 0;
    }
    if notes.is_null() {
        return usize::MAX;
    }
    let Some(required) = note_count.checked_mul(PACKED_NOTE_FIELDS) else {
        return usize::MAX;
    };
    if out.is_null() || capacity < required {
        return required;
    }
    let notes = unsafe { std::slice::from_raw_parts(notes, note_count) };
    let out = unsafe { std::slice::from_raw_parts_mut(out, required) };
    for (note, fields) in notes.iter().zip(out.chunks_exact_mut(PACKED_NOTE_FIELDS)) {
        fields[0] = u64::from(note.track_id);
        fields[1] = u64::from(note.pitch);
        fields[2] = u64::from(note.velocity);
        fields[3] = u64::from(note.midi_channel);
        fields[4] = note.start_sample;
        fields[5] = note.length_samples;
        fields[6] = u64::from(note.probability);
        fields[7] = u64::from(note.region_id);
        fields[8] = u64::from(note.articulation_id);
    }
    required
}
