use crate::midi_quantizer::HirariMidiQuantizerNote;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::Mutex;

const CAPTURE_CAPACITY: usize = 10_000;

#[derive(Clone, Copy)]
struct RawMidiEvent {
    _track_id: u32,
    tick: u64,
    status: u8,
    data1: u8,
    data2: u8,
}

#[derive(Default)]
pub struct RetrospectiveMidiState {
    events: Mutex<VecDeque<RawMidiEvent>>,
}

struct NoteSnapshot {
    notes: Vec<HirariMidiQuantizerNote>,
}

#[no_mangle]
pub extern "C" fn hirari_retrospective_midi_create() -> *mut c_void {
    let mut state = RetrospectiveMidiState::default();
    state.events = Mutex::new(VecDeque::with_capacity(CAPTURE_CAPACITY));
    Box::into_raw(Box::new(state)).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_retrospective_midi_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: the handle is created by `hirari_retrospective_midi_create`.
        unsafe { drop(Box::from_raw(state.cast::<RetrospectiveMidiState>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_retrospective_midi_record(
    state: *mut c_void,
    track_id: u32,
    status: u8,
    data1: u8,
    data2: u8,
    tick: u64,
) {
    let Some(state) = (unsafe { state.cast::<RetrospectiveMidiState>().as_ref() }) else {
        return;
    };
    let mut events = state
        .events
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if events.len() == CAPTURE_CAPACITY {
        events.pop_front();
    }
    events.push_back(RawMidiEvent {
        _track_id: track_id,
        tick,
        status,
        data1,
        data2,
    });
}

#[no_mangle]
pub unsafe extern "C" fn hirari_retrospective_midi_flush_snapshot(
    state: *const c_void,
    current_tick: u64,
    lookback_ticks: u64,
) -> *mut c_void {
    let Some(state) = (unsafe { state.cast::<RetrospectiveMidiState>().as_ref() }) else {
        return std::ptr::null_mut();
    };
    if lookback_ticks == 0 {
        return Box::into_raw(Box::new(NoteSnapshot { notes: Vec::new() })).cast();
    }
    let begin_tick = current_tick.saturating_sub(lookback_ticks);
    let events = state
        .events
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut open = [None::<(u64, u8)>; 16 * 128];
    let mut notes = Vec::new();
    for event in events
        .iter()
        .filter(|event| event.tick >= begin_tick && event.tick <= current_tick)
    {
        let status = event.status & 0xf0;
        let channel = usize::from(event.status & 0x0f);
        let pitch = event.data1 & 0x7f;
        let index = channel * 128 + usize::from(pitch);
        if status == 0x90 && event.data2 > 0 {
            open[index] = Some((event.tick, event.data2));
        } else if status == 0x80 || (status == 0x90 && event.data2 == 0) {
            if let Some((start_tick, velocity)) = open[index].take() {
                if event.tick >= start_tick {
                    notes.push(HirariMidiQuantizerNote {
                        start_tick,
                        length_ticks: event.tick - start_tick,
                        note: pitch,
                        velocity,
                    });
                }
            }
        }
    }
    for (index, active) in open.into_iter().enumerate() {
        if let Some((start_tick, velocity)) = active {
            notes.push(HirariMidiQuantizerNote {
                start_tick,
                length_ticks: current_tick.saturating_sub(start_tick),
                note: (index % 128) as u8,
                velocity,
            });
        }
    }
    Box::into_raw(Box::new(NoteSnapshot { notes })).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_retrospective_midi_snapshot_destroy(snapshot: *mut c_void) {
    if !snapshot.is_null() {
        // SAFETY: the pointer is returned by `hirari_retrospective_midi_flush_snapshot`.
        unsafe { drop(Box::from_raw(snapshot.cast::<NoteSnapshot>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_retrospective_midi_snapshot_count(
    snapshot: *const c_void,
) -> usize {
    unsafe { snapshot.cast::<NoteSnapshot>().as_ref() }.map_or(0, |snapshot| snapshot.notes.len())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_retrospective_midi_snapshot_copy(
    snapshot: *const c_void,
    output: *mut HirariMidiQuantizerNote,
    capacity: usize,
) -> bool {
    let Some(snapshot) = (unsafe { snapshot.cast::<NoteSnapshot>().as_ref() }) else {
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
    fn pairs_note_on_and_note_off_into_tick_based_notes() {
        let state = hirari_retrospective_midi_create();
        unsafe {
            hirari_retrospective_midi_record(state, 1, 0x92, 60, 96, 120);
            hirari_retrospective_midi_record(state, 1, 0x82, 60, 0, 360);
            let snapshot = hirari_retrospective_midi_flush_snapshot(state, 480, 960);
            assert_eq!(hirari_retrospective_midi_snapshot_count(snapshot), 1);
            let mut note = HirariMidiQuantizerNote::default();
            assert!(hirari_retrospective_midi_snapshot_copy(
                snapshot, &mut note, 1
            ));
            assert_eq!(
                note,
                HirariMidiQuantizerNote {
                    start_tick: 120,
                    length_ticks: 240,
                    note: 60,
                    velocity: 96,
                }
            );
            hirari_retrospective_midi_snapshot_destroy(snapshot);
            hirari_retrospective_midi_destroy(state);
        }
    }

    #[test]
    fn unfinished_notes_use_the_capture_end_and_window_is_respected() {
        let state = hirari_retrospective_midi_create();
        unsafe {
            hirari_retrospective_midi_record(state, 1, 0x90, 50, 90, 5);
            hirari_retrospective_midi_record(state, 1, 0x90, 62, 80, 120);
            let snapshot = hirari_retrospective_midi_flush_snapshot(state, 240, 200);
            assert_eq!(hirari_retrospective_midi_snapshot_count(snapshot), 1);
            let mut note = HirariMidiQuantizerNote::default();
            assert!(hirari_retrospective_midi_snapshot_copy(
                snapshot, &mut note, 1
            ));
            assert_eq!(
                (note.start_tick, note.length_ticks, note.note),
                (120, 120, 62)
            );
            hirari_retrospective_midi_snapshot_destroy(snapshot);
            hirari_retrospective_midi_destroy(state);
        }
    }
}
