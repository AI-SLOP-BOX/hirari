use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;

const MAX_NOTES_PER_REGION: usize = 1_000_000;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MidiSequencerNote {
    pub pitch: u8,
    pub velocity: u8,
    pub start_tick: u64,
    pub length: u64,
}

#[derive(Default)]
pub struct MidiSequencerState {
    regions: Mutex<HashMap<u32, Vec<MidiSequencerNote>>>,
}

struct MidiSequencerSnapshot {
    notes: Vec<MidiSequencerNote>,
}

#[no_mangle]
pub extern "C" fn hirari_midi_sequencer_create() -> *mut c_void {
    Box::into_raw(Box::new(MidiSequencerState::default())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_sequencer_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: the opaque pointer was allocated by `hirari_midi_sequencer_create`.
        unsafe { drop(Box::from_raw(state.cast::<MidiSequencerState>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_sequencer_record(
    state: *mut c_void,
    region_id: u32,
    pitch: u8,
    velocity: u8,
    start_tick: u64,
    length: u64,
) -> bool {
    if state.is_null() || pitch > 127 || velocity > 127 || length == 0 {
        return false;
    }
    // SAFETY: state lifetime is owned by the C++ singleton.
    let state = unsafe { &*state.cast::<MidiSequencerState>() };
    let mut regions = state
        .regions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let notes = regions.entry(region_id).or_default();
    if notes.len() >= MAX_NOTES_PER_REGION {
        return false;
    }
    let note = MidiSequencerNote {
        pitch,
        velocity,
        start_tick,
        length,
    };
    let insertion = notes.partition_point(|existing| existing.start_tick <= start_tick);
    notes.insert(insertion, note);
    true
}

/// Copies active notes into caller storage and returns the total number of
/// active notes in the same locked snapshot. A null/zero-capacity call is a
/// count query. If capacity is short, the return value exceeds capacity.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_sequencer_chase(
    state: *const c_void,
    current_tick: u64,
    output: *mut MidiSequencerNote,
    capacity: usize,
) -> usize {
    if state.is_null() {
        return 0;
    }
    let state = unsafe { &*state.cast::<MidiSequencerState>() };
    let regions = state
        .regions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut active_count = 0usize;
    let mut written = 0usize;
    for notes in regions.values() {
        for note in notes {
            if note.start_tick > current_tick {
                break;
            }
            let end = note.start_tick.saturating_add(note.length);
            if current_tick >= end {
                continue;
            }
            if written < capacity && !output.is_null() {
                unsafe { output.add(written).write(*note) };
                written += 1;
            }
            active_count = active_count.saturating_add(1);
        }
    }
    active_count
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_sequencer_clear(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<MidiSequencerState>().as_ref() } {
        state
            .regions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_sequencer_chase_snapshot(
    state: *const c_void,
    current_tick: u64,
) -> *mut c_void {
    let Some(state) = (unsafe { state.cast::<MidiSequencerState>().as_ref() }) else {
        return std::ptr::null_mut();
    };
    let regions = state
        .regions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut notes = Vec::new();
    for region_notes in regions.values() {
        for note in region_notes {
            if note.start_tick > current_tick {
                break;
            }
            if current_tick < note.start_tick.saturating_add(note.length) {
                notes.push(*note);
            }
        }
    }
    Box::into_raw(Box::new(MidiSequencerSnapshot { notes })).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_sequencer_snapshot_destroy(snapshot: *mut c_void) {
    if !snapshot.is_null() {
        // SAFETY: the pointer is returned by `hirari_midi_sequencer_chase_snapshot`.
        unsafe { drop(Box::from_raw(snapshot.cast::<MidiSequencerSnapshot>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_sequencer_snapshot_count(snapshot: *const c_void) -> usize {
    unsafe { snapshot.cast::<MidiSequencerSnapshot>().as_ref() }
        .map_or(0, |snapshot| snapshot.notes.len())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_sequencer_snapshot_copy(
    snapshot: *const c_void,
    output: *mut MidiSequencerNote,
    capacity: usize,
) -> bool {
    let Some(snapshot) = (unsafe { snapshot.cast::<MidiSequencerSnapshot>().as_ref() }) else {
        return false;
    };
    if capacity < snapshot.notes.len() || (!snapshot.notes.is_empty() && output.is_null()) {
        return false;
    }
    if !snapshot.notes.is_empty() {
        unsafe {
            std::ptr::copy_nonoverlapping(snapshot.notes.as_ptr(), output, snapshot.notes.len())
        };
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_sorts_notes_and_chase_obeys_half_open_note_ranges() {
        let state = hirari_midi_sequencer_create();
        unsafe {
            assert!(hirari_midi_sequencer_record(state, 1, 64, 90, 200, 20));
            assert!(hirari_midi_sequencer_record(state, 1, 60, 100, 100, 100));
            assert!(hirari_midi_sequencer_record(state, 2, 48, 80, 150, 50));
            let mut output = [MidiSequencerNote::default(); 4];
            let count = hirari_midi_sequencer_chase(state, 199, output.as_mut_ptr(), output.len());
            assert_eq!(count, 2);
            let mut pitches = output[..count]
                .iter()
                .map(|note| note.pitch)
                .collect::<Vec<_>>();
            pitches.sort_unstable();
            assert_eq!(pitches, vec![48, 60]);
            assert_eq!(
                hirari_midi_sequencer_chase(state, 200, output.as_mut_ptr(), output.len()),
                1
            );
            assert_eq!(output[0].pitch, 64);
            hirari_midi_sequencer_destroy(state);
        }
    }

    #[test]
    fn rejects_invalid_notes_and_saturates_end_tick() {
        let state = hirari_midi_sequencer_create();
        unsafe {
            assert!(!hirari_midi_sequencer_record(state, 7, 128, 90, 0, 10));
            assert!(!hirari_midi_sequencer_record(state, 7, 60, 90, 0, 0));
            assert!(hirari_midi_sequencer_record(
                state,
                7,
                60,
                90,
                u64::MAX - 2,
                10
            ));
            let mut output = [MidiSequencerNote::default(); 1];
            assert_eq!(
                hirari_midi_sequencer_chase(state, u64::MAX, output.as_mut_ptr(), output.len(),),
                0
            );
            assert_eq!(
                hirari_midi_sequencer_chase(state, u64::MAX - 1, output.as_mut_ptr(), output.len(),),
                1
            );
            hirari_midi_sequencer_clear(state);
            assert_eq!(
                hirari_midi_sequencer_chase(state, u64::MAX, output.as_mut_ptr(), output.len(),),
                0
            );
            hirari_midi_sequencer_destroy(state);
        }
    }

    #[test]
    fn short_output_capacity_reports_required_count_without_overrun() {
        let state = hirari_midi_sequencer_create();
        unsafe {
            for region in 0..3 {
                assert!(hirari_midi_sequencer_record(state, region, 60, 90, 0, 100));
            }
            let mut output = [MidiSequencerNote::default(); 1];
            assert_eq!(
                hirari_midi_sequencer_chase(state, 0, output.as_mut_ptr(), 1),
                3
            );
            assert_eq!(output[0].pitch, 60);
            assert_eq!(
                hirari_midi_sequencer_chase(state, 0, std::ptr::null_mut(), 0),
                3
            );
            hirari_midi_sequencer_destroy(state);
        }
    }

    #[test]
    fn snapshots_remain_stable_after_live_state_changes() {
        let state = hirari_midi_sequencer_create();
        unsafe {
            assert!(hirari_midi_sequencer_record(state, 2, 67, 88, 100, 50));
            let snapshot = hirari_midi_sequencer_chase_snapshot(state, 120);
            assert!(!snapshot.is_null());
            hirari_midi_sequencer_clear(state);
            assert_eq!(hirari_midi_sequencer_snapshot_count(snapshot), 1);
            let mut note = MidiSequencerNote::default();
            assert!(hirari_midi_sequencer_snapshot_copy(snapshot, &mut note, 1));
            assert_eq!(note.pitch, 67);
            hirari_midi_sequencer_snapshot_destroy(snapshot);
            hirari_midi_sequencer_destroy(state);
        }
    }
}
