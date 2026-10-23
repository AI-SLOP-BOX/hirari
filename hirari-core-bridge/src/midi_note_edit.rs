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

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;

#[derive(Default)]
struct MidiSnapshot {
    notes_by_start: Vec<ScheduledMidiNoteFfi>,
    indices_by_end: Vec<usize>,
}

struct MidiSnapshotStore {
    slots: [UnsafeCell<MidiSnapshot>; 3],
    active: AtomicUsize,
    readers: [AtomicUsize; 3],
    revision: AtomicU64,
    writer: Mutex<()>,
}

// Writers mutate only an inactive slot whose reader count is zero. Readers
// increment before inspecting the revision and never touch the vectors until
// the active slot and revision are confirmed stable.
unsafe impl Send for MidiSnapshotStore {}
unsafe impl Sync for MidiSnapshotStore {}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MidiSnapshotView {
    pub notes: *const ScheduledMidiNoteFfi,
    pub note_count: usize,
    pub indices_by_end: *const usize,
    pub end_count: usize,
    pub slot: usize,
    pub revision: u64,
}
const _: () = assert!(std::mem::size_of::<MidiSnapshotView>() == 48);

impl MidiSnapshotStore {
    fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| UnsafeCell::new(MidiSnapshot::default())),
            active: AtomicUsize::new(0),
            readers: std::array::from_fn(|_| AtomicUsize::new(0)),
            revision: AtomicU64::new(0),
            writer: Mutex::new(()),
        }
    }

    fn publish(&self, snapshot: MidiSnapshot) {
        let _writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let slot = loop {
            let active = self.active.load(Ordering::Acquire);
            if let Some(slot) = (1..3)
                .map(|offset| (active + offset) % 3)
                .find(|&slot| self.readers[slot].load(Ordering::Acquire) == 0)
            {
                break slot;
            }
            thread::yield_now();
        };
        self.revision.fetch_add(1, Ordering::AcqRel);
        unsafe { *self.slots[slot].get() = snapshot };
        self.active.store(slot, Ordering::Release);
        self.revision.fetch_add(1, Ordering::Release);
    }

    fn pin(&self) -> Option<MidiSnapshotView> {
        for _ in 0..8 {
            let before = self.revision.load(Ordering::Acquire);
            if before & 1 != 0 {
                continue;
            }
            let slot = self.active.load(Ordering::Acquire);
            self.readers[slot].fetch_add(1, Ordering::AcqRel);
            let active = self.active.load(Ordering::Acquire);
            let after = self.revision.load(Ordering::Acquire);
            if active == slot && before == after && after & 1 == 0 {
                let snapshot = unsafe { &*self.slots[slot].get() };
                return Some(MidiSnapshotView {
                    notes: snapshot.notes_by_start.as_ptr(),
                    note_count: snapshot.notes_by_start.len(),
                    indices_by_end: snapshot.indices_by_end.as_ptr(),
                    end_count: snapshot.indices_by_end.len(),
                    slot,
                    revision: after,
                });
            }
            self.readers[slot].fetch_sub(1, Ordering::Release);
        }
        None
    }

    fn release(&self, slot: usize) {
        if let Some(readers) = self.readers.get(slot) {
            readers.fetch_sub(1, Ordering::Release);
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_midi_snapshot_store_create() -> *mut c_void {
    Box::into_raw(Box::new(MidiSnapshotStore::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_snapshot_store_destroy(store: *mut c_void) {
    if !store.is_null() {
        drop(unsafe { Box::from_raw(store.cast::<MidiSnapshotStore>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_snapshot_store_publish(
    store: *mut c_void,
    notes: *const ScheduledMidiNoteFfi,
    count: usize,
) -> bool {
    let Some(store) = (unsafe { store.cast::<MidiSnapshotStore>().as_ref() }) else {
        return false;
    };
    if count > 100_000 || (count > 0 && notes.is_null()) {
        return false;
    }
    let mut notes_by_start = if count == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(notes, count) }.to_vec()
    };
    let mut indices_by_end = vec![0; count];
    if !unsafe {
        hirari_midi_snapshot_build(
            notes_by_start.as_mut_ptr(),
            count,
            indices_by_end.as_mut_ptr(),
            indices_by_end.len(),
        )
    } {
        return false;
    }
    store.publish(MidiSnapshot {
        notes_by_start,
        indices_by_end,
    });
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_snapshot_store_pin(
    store: *const c_void,
    output: *mut MidiSnapshotView,
) -> bool {
    let Some(store) = (unsafe { store.cast::<MidiSnapshotStore>().as_ref() }) else {
        return false;
    };
    let Some(view) = store.pin() else {
        return false;
    };
    if output.is_null() {
        store.release(view.slot);
        return false;
    }
    unsafe { output.write(view) };
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_snapshot_store_release(store: *const c_void, slot: usize) {
    if let Some(store) = unsafe { store.cast::<MidiSnapshotStore>().as_ref() } {
        store.release(slot);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_snapshot_store_revision(store: *const c_void) -> u64 {
    unsafe { store.cast::<MidiSnapshotStore>().as_ref() }
        .map_or(0, |store| store.revision.load(Ordering::Acquire))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_scheduled_note_end(note: *const ScheduledMidiNoteFfi) -> u64 {
    let Some(note) = (unsafe { note.as_ref() }) else {
        return 0;
    };
    note.start_sample.saturating_add(note.length_samples)
}

const UNCHANGED: i64 = -1;
const INVALID: i64 = -2;

/// Validates and builds both playback orderings for the immutable scheduled
/// note snapshot. Rust owns the stable ordering policy; native code keeps the
/// snapshot buffers because its real-time callback dispatches to Track objects.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_snapshot_build(
    notes: *mut ScheduledMidiNoteFfi,
    count: usize,
    indices_by_end: *mut usize,
    index_capacity: usize,
) -> bool {
    if count > 100_000
        || (count > 0 && (notes.is_null() || indices_by_end.is_null()))
        || index_capacity < count
    {
        return false;
    }
    if count == 0 {
        return true;
    }
    let notes = unsafe { std::slice::from_raw_parts_mut(notes, count) };
    if notes.iter().any(|note| {
        note.pitch > 127
            || note.velocity == 0
            || note.midi_channel > 15
            || note.probability > 100
            || note.length_samples == 0
            || note.start_sample > u64::MAX - note.length_samples
    }) {
        return false;
    }
    notes.sort_by_key(|note| note.start_sample);
    let indices = unsafe { std::slice::from_raw_parts_mut(indices_by_end, count) };
    for (index, value) in indices.iter_mut().enumerate() {
        *value = index;
    }
    indices.sort_by_key(|&index| {
        notes[index]
            .start_sample
            .saturating_add(notes[index].length_samples)
    });
    true
}

/// Inserts or updates a legacy track-scoped note in the caller's spare slot.
/// Returns the new note count, -1 for no change, or -2 for invalid input.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_note_upsert(
    notes: *mut ScheduledMidiNoteFfi,
    count: usize,
    capacity: usize,
    track_id: u32,
    pitch: u8,
    velocity: u8,
    start_sample: u64,
    length_samples: u64,
) -> i64 {
    if notes.is_null()
        || count > 100_000
        || capacity <= count
        || pitch > 127
        || velocity == 0
        || length_samples == 0
        || start_sample.checked_add(length_samples).is_none()
    {
        return INVALID;
    }
    let notes = unsafe { std::slice::from_raw_parts_mut(notes, capacity) };
    if let Some(note) = notes[..count].iter_mut().find(|note| {
        note.track_id == track_id
            && note.pitch == pitch
            && note.midi_channel == 0
            && note.start_sample == start_sample
            && note.region_id == 0
    }) {
        if note.velocity == velocity && note.length_samples == length_samples {
            return UNCHANGED;
        }
        note.velocity = velocity;
        note.length_samples = length_samples;
        return count as i64;
    }
    if count == 100_000 {
        return INVALID;
    }
    notes[count] = ScheduledMidiNoteFfi {
        track_id,
        pitch,
        velocity,
        midi_channel: 0,
        articulation_id: 0,
        start_sample,
        length_samples,
        probability: 100,
        region_id: 0,
    };
    (count + 1) as i64
}

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
