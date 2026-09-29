//! MIDI region note editing operations used by the native editor adapter.

use std::ffi::c_void;
use std::sync::Mutex;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HirariMidiBeatNote {
    pub pitch: u8,
    pub velocity: u8,
    pub start_beat: f64,
    pub length_beats: f64,
}

struct MidiRegionNoteState {
    notes: Mutex<Vec<HirariMidiBeatNote>>,
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_region_state_create(
    notes: *const c_void,
    count: usize,
) -> *mut c_void {
    if count > 1_000_000 || (count != 0 && notes.is_null()) {
        return std::ptr::null_mut();
    }
    let notes = if count == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(notes.cast::<HirariMidiBeatNote>(), count) }.to_vec()
    };
    Box::into_raw(Box::new(MidiRegionNoteState {
        notes: Mutex::new(notes),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_region_state_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<MidiRegionNoteState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_region_state_count(state: *const c_void) -> usize {
    unsafe { state.cast::<MidiRegionNoteState>().as_ref() }
        .and_then(|state| state.notes.lock().ok().map(|notes| notes.len()))
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_region_state_copy(
    state: *const c_void,
    output: *mut c_void,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { state.cast::<MidiRegionNoteState>().as_ref() }) else {
        return 0;
    };
    let Ok(notes) = state.notes.lock() else {
        return 0;
    };
    if output.is_null() || capacity < notes.len() {
        return notes.len();
    }
    unsafe {
        std::ptr::copy_nonoverlapping(
            notes.as_ptr(),
            output.cast::<HirariMidiBeatNote>(),
            notes.len(),
        )
    };
    notes.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_region_state_add(
    state: *mut c_void,
    note: *const c_void,
) -> bool {
    let Some(state) = (unsafe { state.cast::<MidiRegionNoteState>().as_mut() }) else {
        return false;
    };
    if note.is_null() {
        return false;
    }
    let Ok(mut notes) = state.notes.lock() else {
        return false;
    };
    if notes.len() >= 1_000_000 {
        return false;
    }
    notes.push(unsafe { note.cast::<HirariMidiBeatNote>().read() });
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_region_state_remove(state: *mut c_void, index: usize) -> bool {
    let Some(state) = (unsafe { state.cast::<MidiRegionNoteState>().as_mut() }) else {
        return false;
    };
    let Ok(mut notes) = state.notes.lock() else {
        return false;
    };
    if index >= notes.len() {
        return false;
    }
    notes.remove(index);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_region_state_update(
    state: *mut c_void,
    index: usize,
    note: *const c_void,
) -> bool {
    let Some(state) = (unsafe { state.cast::<MidiRegionNoteState>().as_mut() }) else {
        return false;
    };
    if note.is_null() {
        return false;
    }
    let Ok(mut notes) = state.notes.lock() else {
        return false;
    };
    if index >= notes.len() {
        return false;
    }
    let note = unsafe { note.cast::<HirariMidiBeatNote>().read() };
    if !note.start_beat.is_finite()
        || !note.length_beats.is_finite()
        || note.start_beat < 0.0
        || note.length_beats <= 0.0
    {
        return false;
    }
    notes[index] = note;
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_region_state_replace(
    state: *mut c_void,
    notes: *const c_void,
    count: usize,
) -> bool {
    let Some(state) = (unsafe { state.cast::<MidiRegionNoteState>().as_mut() }) else {
        return false;
    };
    if count > 1_000_000 || (count > 0 && notes.is_null()) {
        return false;
    }
    let candidate = if count == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(notes.cast::<HirariMidiBeatNote>(), count) }.to_vec()
    };
    if candidate.iter().any(|note| {
        !note.start_beat.is_finite()
            || !note.length_beats.is_finite()
            || note.start_beat < 0.0
            || note.length_beats <= 0.0
    }) {
        return false;
    }
    let Ok(mut current) = state.notes.lock() else {
        return false;
    };
    *current = candidate;
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_region_state_transpose(
    state: *mut c_void,
    semitones: i32,
    selection_start: f64,
    selection_end: f64,
) {
    let Some(state) = (unsafe { state.cast::<MidiRegionNoteState>().as_mut() }) else {
        return;
    };
    let Ok(mut notes) = state.notes.lock() else {
        return;
    };
    for note in &mut *notes {
        if selection_start >= 0.0
            && (note.start_beat < selection_start
                || (selection_end >= 0.0 && note.start_beat > selection_end))
        {
            continue;
        }
        note.pitch = (i32::from(note.pitch) + semitones).clamp(0, 127) as u8;
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_region_state_quantize(
    state: *mut c_void,
    grid: f64,
    strength: f64,
    selection_start: f64,
    selection_end: f64,
) -> bool {
    let Some(state) = (unsafe { state.cast::<MidiRegionNoteState>().as_mut() }) else {
        return false;
    };
    let Ok(mut notes) = state.notes.lock() else {
        return false;
    };
    unsafe {
        hirari_midi_region_quantize(
            notes.as_mut_ptr().cast(),
            notes.len(),
            grid,
            strength,
            selection_start,
            selection_end,
        )
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_region_state_remove_notes_at(
    state: *mut c_void,
    beat: f64,
    pitch: i32,
    tolerance: f64,
) {
    let Some(state) = (unsafe { state.cast::<MidiRegionNoteState>().as_mut() }) else {
        return;
    };
    let Ok(mut notes) = state.notes.lock() else {
        return;
    };
    let count = notes.len();
    let retained = unsafe {
        hirari_midi_region_remove_notes_at(notes.as_mut_ptr().cast(), count, beat, pitch, tolerance)
    };
    notes.truncate(retained);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_region_state_set_muted_at(
    state: *mut c_void,
    beat: f64,
    pitch: i32,
    muted: bool,
) {
    let Some(state) = (unsafe { state.cast::<MidiRegionNoteState>().as_mut() }) else {
        return;
    };
    let Ok(mut notes) = state.notes.lock() else {
        return;
    };
    unsafe {
        hirari_midi_region_set_muted_at(notes.as_mut_ptr().cast(), notes.len(), beat, pitch, muted)
    };
}

/// Quantizes selected note starts and stably orders the region by start beat.
///
/// # Safety
/// `notes` must point to `count` writable `HirariMidiBeatNote` values.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_region_quantize(
    notes: *mut c_void,
    count: usize,
    grid: f64,
    strength: f64,
    selection_start: f64,
    selection_end: f64,
) -> bool {
    if notes.is_null() || count == 0 || !grid.is_finite() || grid <= 0.0 || !strength.is_finite() {
        return false;
    }
    let notes =
        unsafe { std::slice::from_raw_parts_mut(notes.cast::<HirariMidiBeatNote>(), count) };
    let strength = strength.clamp(0.0, 1.0);
    for note in notes.iter_mut() {
        if selection_start >= 0.0
            && (note.start_beat < selection_start
                || (selection_end >= 0.0 && note.start_beat > selection_end))
        {
            continue;
        }
        let snapped = (note.start_beat / grid).round() * grid;
        note.start_beat += (snapped - note.start_beat) * strength;
    }
    notes.sort_by(|left, right| {
        left.start_beat
            .partial_cmp(&right.start_beat)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_region_transpose(
    notes: *mut c_void,
    count: usize,
    semitones: i32,
    selection_start: f64,
    selection_end: f64,
) {
    if notes.is_null() || count == 0 {
        return;
    }
    let notes =
        unsafe { std::slice::from_raw_parts_mut(notes.cast::<HirariMidiBeatNote>(), count) };
    for note in notes {
        if selection_start >= 0.0
            && (note.start_beat < selection_start
                || (selection_end >= 0.0 && note.start_beat > selection_end))
        {
            continue;
        }
        note.pitch = (i32::from(note.pitch) + semitones).clamp(0, 127) as u8;
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_region_remove_notes_at(
    notes: *mut c_void,
    count: usize,
    beat: f64,
    pitch: i32,
    tolerance: f64,
) -> usize {
    if notes.is_null() {
        return 0;
    }
    let pitch = pitch.clamp(0, 127) as u8;
    let tolerance = tolerance.max(0.0);
    let notes = notes.cast::<HirariMidiBeatNote>();
    let mut retained = 0;
    for index in 0..count {
        let note = unsafe { notes.add(index).read() };
        if note.pitch == pitch && (note.start_beat - beat).abs() <= tolerance {
            continue;
        }
        unsafe { notes.add(retained).write(note) };
        retained += 1;
    }
    retained
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_region_set_muted_at(
    notes: *mut c_void,
    count: usize,
    beat: f64,
    pitch: i32,
    muted: bool,
) {
    if notes.is_null() || count == 0 {
        return;
    }
    let notes =
        unsafe { std::slice::from_raw_parts_mut(notes.cast::<HirariMidiBeatNote>(), count) };
    let pitch = pitch.clamp(0, 127) as u8;
    for note in notes {
        if note.pitch == pitch && (note.start_beat - beat).abs() <= 0.125 {
            note.velocity = if muted { 0 } else { note.velocity.max(1) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{hirari_midi_region_quantize, HirariMidiBeatNote};

    unsafe extern "C" {
        fn hirari_midi_region_cpp_wrapper_smoke() -> bool;
    }

    #[test]
    fn quantizes_only_selected_note_starts_and_preserves_other_fields() {
        let mut notes = [
            HirariMidiBeatNote {
                pitch: 72,
                velocity: 88,
                start_beat: 2.37,
                length_beats: 0.4,
            },
            HirariMidiBeatNote {
                pitch: 60,
                velocity: 99,
                start_beat: 0.12,
                length_beats: 1.0,
            },
            HirariMidiBeatNote {
                pitch: 64,
                velocity: 77,
                start_beat: 1.43,
                length_beats: 0.25,
            },
        ];
        assert!(unsafe {
            hirari_midi_region_quantize(notes.as_mut_ptr().cast(), notes.len(), 0.5, 0.5, 1.0, 2.5)
        });
        assert_eq!(notes[0].pitch, 60);
        assert_eq!(notes[0].start_beat, 0.12);
        assert_eq!(notes[1].pitch, 64);
        assert!((notes[1].start_beat - 1.465).abs() < 1.0e-12);
        assert_eq!(notes[2].pitch, 72);
        assert!((notes[2].start_beat - 2.435).abs() < 1.0e-12);
        assert_eq!(notes[2].length_beats, 0.4);
        assert_eq!(notes[2].velocity, 88);
    }

    #[test]
    fn invalid_quantization_does_not_mutate_notes() {
        assert_eq!(std::mem::size_of::<HirariMidiBeatNote>(), 24);
        assert_eq!(std::mem::offset_of!(HirariMidiBeatNote, start_beat), 8);
        let original = HirariMidiBeatNote {
            pitch: 65,
            velocity: 90,
            start_beat: 1.2,
            length_beats: 0.3,
        };
        let mut note = original;
        assert!(!unsafe {
            hirari_midi_region_quantize(&mut note as *mut _ as *mut _, 1, f64::NAN, 1.0, -1.0, -1.0)
        });
        assert_eq!(note, original);
    }

    #[test]
    fn transpose_mute_and_remove_share_the_editor_note_abi() {
        let mut notes = [
            HirariMidiBeatNote {
                pitch: 60,
                velocity: 80,
                start_beat: 0.2,
                length_beats: 0.5,
            },
            HirariMidiBeatNote {
                pitch: 124,
                velocity: 0,
                start_beat: 0.7,
                length_beats: 0.25,
            },
            HirariMidiBeatNote {
                pitch: 70,
                velocity: 90,
                start_beat: 0.8,
                length_beats: 0.25,
            },
        ];
        unsafe {
            super::hirari_midi_region_transpose(
                notes.as_mut_ptr().cast(),
                notes.len(),
                10,
                0.5,
                0.75,
            );
            super::hirari_midi_region_set_muted_at(
                notes.as_mut_ptr().cast(),
                notes.len(),
                0.7,
                200,
                true,
            );
            super::hirari_midi_region_set_muted_at(
                notes.as_mut_ptr().cast(),
                notes.len(),
                0.7,
                127,
                false,
            );
        }
        assert_eq!(notes[0].pitch, 60);
        assert_eq!((notes[1].pitch, notes[1].velocity), (127, 1));
        assert_eq!(notes[2].pitch, 70);

        let retained = unsafe {
            super::hirari_midi_region_remove_notes_at(
                notes.as_mut_ptr().cast(),
                notes.len(),
                0.7,
                200,
                -1.0,
            )
        };
        assert_eq!(retained, 2);
        assert_eq!(notes[0].pitch, 60);
        assert_eq!(notes[1].pitch, 70);
    }

    #[test]
    fn cpp_midi_region_adapter_uses_rust_owned_note_storage() {
        assert!(unsafe { hirari_midi_region_cpp_wrapper_smoke() });
    }
}
