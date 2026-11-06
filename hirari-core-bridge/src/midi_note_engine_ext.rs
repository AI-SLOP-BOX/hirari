//! Rust-side entry points for the MIDI-note edits the C++ engine and its
//! `AudioEngine` wrapper used to relay. The note store, its range edits and
//! the packed wire format are Rust-owned; only the store handle it renders
//! with crosses back here.

use std::ffi::c_void;

use crate::ffi;
use crate::midi_note_edit::{
    hirari_midi_notes_edit_range, hirari_midi_notes_pack, hirari_midi_notes_replace,
    hirari_midi_notes_snapshot_take, hirari_midi_notes_snapshot_take_free,
    hirari_midi_notes_unpack, hirari_midi_notes_upsert, ScheduledMidiNoteFfi,
};

fn store(engine: &crate::AudioEngine) -> *mut c_void {
    let core: *const crate::HirariUnifiedEngine = crate::get_unified_engine(engine);
    unsafe { (*core).midi_snapshot_store() as *mut c_void }
}

pub trait MidiNoteEngineExt {
    fn clear_midi_notes(&self);
    fn replace_midi_notes(&self, packed: Vec<u64>, record_undo: bool) -> bool;
    fn midi_notes_snapshot(&self) -> Vec<u64>;
    fn remove_midi_notes_range(&self, track_id: u32, start_sample: u64, end_sample: u64) -> bool;
    fn transpose_midi_notes_range(
        &self,
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
        semitones: i32,
    ) -> bool;
    fn move_midi_notes_range(
        &self,
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
        delta_samples: i64,
    ) -> bool;
    fn set_midi_note(
        &self,
        track_id: u32,
        pitch: u8,
        velocity: u8,
        start_sample: u64,
        length_samples: u64,
    );
}

impl MidiNoteEngineExt for crate::AudioEngine {
    fn clear_midi_notes(&self) {
        self.replace_midi_notes(Vec::new(), true);
    }

    fn replace_midi_notes(&self, packed: Vec<u64>, record_undo: bool) -> bool {
        let count =
            unsafe { hirari_midi_notes_unpack(packed.as_ptr(), packed.len(), std::ptr::null_mut(), 0) };
        if count == usize::MAX {
            return false;
        }
        let mut notes = vec![ScheduledMidiNoteFfi::default(); count];
        if count > 0
            && unsafe {
                hirari_midi_notes_unpack(packed.as_ptr(), packed.len(), notes.as_mut_ptr(), count)
            } != count
        {
            return false;
        }
        unsafe {
            hirari_midi_notes_replace(
                store(self),
                notes.as_ptr(),
                count,
                record_undo,
                self.undo_state() as *mut c_void,
                self.log_state() as *const c_void,
            )
        }
    }

    fn midi_notes_snapshot(&self) -> Vec<u64> {
        let mut count = 0usize;
        let notes = unsafe { hirari_midi_notes_snapshot_take(store(self), &mut count) };
        if notes.is_null() {
            return Vec::new();
        }
        let required = unsafe { hirari_midi_notes_pack(notes, count, std::ptr::null_mut(), 0) };
        let mut packed = Vec::new();
        if required != usize::MAX {
            packed.resize(required, 0);
            unsafe { hirari_midi_notes_pack(notes, count, packed.as_mut_ptr(), required) };
        }
        unsafe { hirari_midi_notes_snapshot_take_free(notes, count) };
        packed
    }

    fn remove_midi_notes_range(&self, track_id: u32, start_sample: u64, end_sample: u64) -> bool {
        self.edit_midi_notes_range(track_id, start_sample, end_sample, 0, 0)
    }

    fn transpose_midi_notes_range(
        &self,
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
        semitones: i32,
    ) -> bool {
        self.edit_midi_notes_range(track_id, start_sample, end_sample, 1, i64::from(semitones))
    }

    fn move_midi_notes_range(
        &self,
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
        delta_samples: i64,
    ) -> bool {
        self.edit_midi_notes_range(track_id, start_sample, end_sample, 2, delta_samples)
    }

    fn set_midi_note(
        &self,
        track_id: u32,
        pitch: u8,
        velocity: u8,
        start_sample: u64,
        length_samples: u64,
    ) {
        unsafe {
            hirari_midi_notes_upsert(
                store(self),
                track_id,
                pitch,
                velocity,
                start_sample,
                length_samples,
                self.undo_state() as *mut c_void,
                self.log_state() as *const c_void,
            );
        }
    }
}

impl crate::AudioEngine {
    fn edit_midi_notes_range(
        &self,
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
        operation: u32,
        amount: i64,
    ) -> bool {
        unsafe {
            hirari_midi_notes_edit_range(
                store(self),
                track_id,
                start_sample,
                end_sample,
                operation,
                amount,
                self.undo_state() as *mut c_void,
                self.log_state() as *const c_void,
            )
        }
    }
}
