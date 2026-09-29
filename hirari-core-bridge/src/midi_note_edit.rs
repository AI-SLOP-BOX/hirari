#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScheduledMidiNoteFfi {
    pub track_id: u32,
    pub pitch: u8,
    pub velocity: u8,
    pub midi_channel: u8,
    pub articulation_id: u8,
    pub start_sample: u64,
    pub length_samples: u64,
    pub probability: u8,
    pub region_id: u32,
}

const UNCHANGED: i64 = -1;
const INVALID: i64 = -2;

pub type ScheduledMidiEventCallback = unsafe extern "C" fn(
    context: *mut std::ffi::c_void,
    note: *const ScheduledMidiNoteFfi,
    sample_offset: u64,
    note_off: u8,
);

fn probability_passes(note: &ScheduledMidiNoteFfi, pass: u64) -> bool {
    if note.probability >= 100 {
        return true;
    }
    if note.probability == 0 {
        return false;
    }
    let mut value = note.start_sample
        ^ (u64::from(note.track_id) << 32)
        ^ (u64::from(note.region_id) << 7)
        ^ (u64::from(note.pitch) << 24)
        ^ (u64::from(note.midi_channel) << 16)
        ^ pass.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^= value >> 31;
    value % 100 < u64::from(note.probability)
}

/// Selects scheduled MIDI note events for one audio block. Dispatch remains
/// in the host because it targets native Track instances; range search,
/// probability, reconciliation, and boundary rules are implemented here.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_schedule_block(
    notes: *const ScheduledMidiNoteFfi,
    note_count: usize,
    indices_by_end: *const usize,
    end_count: usize,
    playhead: u64,
    frames: u32,
    playback_active: u8,
    reconcile: u8,
    pass: u64,
    context: *mut std::ffi::c_void,
    callback: Option<ScheduledMidiEventCallback>,
) {
    let Some(callback) = callback else { return };
    if note_count == 0 || notes.is_null() || (end_count != 0 && indices_by_end.is_null()) {
        return;
    }
    let Some(block_end) = playhead.checked_add(u64::from(frames)) else {
        return;
    };
    let notes = unsafe { std::slice::from_raw_parts(notes, note_count) };
    let end_indices = if end_count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(indices_by_end, end_count) }
    };
    if end_indices.iter().any(|&index| index >= notes.len()) {
        return;
    }
    if playback_active == 0 {
        return;
    }

    if reconcile != 0 {
        for note in notes {
            let note_end = note.start_sample.saturating_add(note.length_samples);
            if probability_passes(note, pass) && note.start_sample < playhead && note_end > playhead
            {
                unsafe { callback(context, note, 0, 0) };
            }
        }
    }

    let first_note = notes.partition_point(|note| note.start_sample < playhead);
    for note in &notes[first_note..] {
        if note.start_sample >= block_end {
            break;
        }
        if probability_passes(note, pass) {
            unsafe { callback(context, note, note.start_sample - playhead, 0) };
        }
    }

    let first_end = end_indices.partition_point(|&index| {
        notes[index]
            .start_sample
            .saturating_add(notes[index].length_samples)
            < playhead
    });
    for &index in &end_indices[first_end..] {
        let note = &notes[index];
        let note_end = note.start_sample.saturating_add(note.length_samples);
        if note_end >= block_end {
            break;
        }
        if probability_passes(note, pass) {
            unsafe { callback(context, note, note_end - playhead, 1) };
        }
    }
}

/// Applies the native MIDI range edit in place. `operation` is 0=remove,
/// 1=transpose and 2=move. Returns the resulting note count, UNCHANGED, or
/// INVALID. The input layout is shared with the C++ ScheduledMidiNote ABI.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_edit_range(
    notes: *mut ScheduledMidiNoteFfi,
    count: usize,
    track_id: u32,
    start_sample: u64,
    end_sample: u64,
    operation: u32,
    amount: i64,
) -> i64 {
    if (count != 0 && notes.is_null()) || track_id == 0 || start_sample >= end_sample {
        return INVALID;
    }
    if (operation == 1 && !(-127..=127).contains(&amount)) || operation > 2 {
        return INVALID;
    }
    let notes = if count == 0 {
        &mut [][..]
    } else {
        unsafe { std::slice::from_raw_parts_mut(notes, count) }
    };
    let overlaps = |note: &ScheduledMidiNoteFfi| {
        note.track_id == track_id
            && note.start_sample < end_sample
            && note.start_sample.saturating_add(note.length_samples) > start_sample
    };
    let matched = notes.iter().filter(|note| overlaps(note)).count();
    if matched == 0 {
        return UNCHANGED;
    }

    if operation == 1 {
        for note in notes.iter().filter(|note| overlaps(note)) {
            let pitch = i64::from(note.pitch) + amount;
            if !(0..=127).contains(&pitch) {
                return INVALID;
            }
        }
    } else if operation == 2 {
        for note in notes.iter().filter(|note| overlaps(note)) {
            let valid = if amount < 0 {
                note.start_sample
                    .checked_sub(amount.unsigned_abs())
                    .is_some()
            } else {
                note.start_sample.checked_add(amount as u64).is_some()
            };
            if !valid {
                return INVALID;
            }
        }
    }

    if operation == 0 {
        let mut kept = 0;
        for read in 0..notes.len() {
            if !overlaps(&notes[read]) {
                if kept != read {
                    notes[kept] = notes[read];
                }
                kept += 1;
            }
        }
        return kept as i64;
    }
    for note in notes.iter_mut().filter(|note| overlaps(note)) {
        if operation == 1 {
            note.pitch = (i64::from(note.pitch) + amount) as u8;
        } else if amount < 0 {
            note.start_sample -= amount.unsigned_abs();
        } else {
            note.start_sample += amount as u64;
        }
    }
    count as i64
}
