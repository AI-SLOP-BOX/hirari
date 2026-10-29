#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
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
    // Serializes the read-modify-publish sequence of one note edit, as the
    // engine's control-side mutex did. Plain snapshots do not take it.
    edit: Mutex<()>,
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
            edit: Mutex::new(()),
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

    /// Copies the published notes out of the stable slot. The engine's control
    /// path waited for a pin rather than failing, so this retries the same way.
    fn snapshot_notes(&self) -> Vec<ScheduledMidiNoteFfi> {
        let view = loop {
            if let Some(view) = self.pin() {
                break view;
            }
            thread::yield_now();
        };
        let notes = if view.note_count == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(view.notes, view.note_count) }.to_vec()
        };
        self.release(view.slot);
        notes
    }
}

/// Builds and publishes one immutable note snapshot, applying the same
/// validation and ordering the standalone publish entry point used.
fn publish_snapshot(store: &MidiSnapshotStore, notes: &[ScheduledMidiNoteFfi]) -> bool {
    if notes.len() > 100_000 {
        return false;
    }
    let mut notes_by_start = notes.to_vec();
    let mut indices_by_end = vec![0; notes_by_start.len()];
    if !unsafe {
        hirari_midi_snapshot_build(
            notes_by_start.as_mut_ptr(),
            notes_by_start.len(),
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

/// Replays one history snapshot under the same edit mutex the forward path
/// held. The native undo/redo lambdas published through that lock.
fn replay_snapshot(store: *const MidiSnapshotStore, notes: &[ScheduledMidiNoteFfi]) {
    let _edit = unsafe { &*store }
        .edit
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _ = unsafe { publish_snapshot(&*store, notes) };
}

/// Adopts `notes` as the scheduled note list and, when asked, records one
/// non-coalescing "Replace MIDI Notes" entry. The native method held its
/// control-side mutex across the before-read and the publish, then recorded
/// outside the lock; the edit mutex reproduces that scope.
fn replace_notes(
    store: &MidiSnapshotStore,
    notes: Vec<ScheduledMidiNoteFfi>,
    record_undo: bool,
    undo: *mut c_void,
    log: *const c_void,
) -> bool {
    if notes.len() > 100_000 {
        return false;
    }
    let before = {
        let _edit = store.edit.lock().unwrap_or_else(|error| error.into_inner());
        let before = store.snapshot_notes();
        if !publish_snapshot(store, &notes) {
            return false;
        }
        before
    };
    if record_undo {
        let store = store as *const MidiSnapshotStore;
        let after = notes;
        crate::record_external_action(
            undo,
            log,
            "Replace MIDI Notes",
            move || replay_snapshot(store, &before),
            move || replay_snapshot(store, &after),
        );
    }
    true
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
    let notes = if count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(notes, count) }
    };
    publish_snapshot(store, notes)
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

/// # Safety
/// `store` must be null or a handle returned by
/// `hirari_midi_snapshot_store_create` that is still alive.
unsafe fn store_ref<'a>(store: *mut c_void) -> Option<&'a MidiSnapshotStore> {
    unsafe { store.cast::<MidiSnapshotStore>().as_ref() }
}

/// Copies the published note list. Returns the current count, which is larger
/// than `capacity` when the caller has to grow and retry; a null `output` is
/// the count query.
///
/// # Safety
/// `output` must be writable for `capacity` notes.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_notes_snapshot_copy(
    store: *const c_void,
    output: *mut ScheduledMidiNoteFfi,
    capacity: usize,
) -> usize {
    let Some(store) = (unsafe { store.cast::<MidiSnapshotStore>().as_ref() }) else {
        return 0;
    };
    let notes = store.snapshot_notes();
    if output.is_null() || capacity < notes.len() {
        return notes.len();
    }
    if !notes.is_empty() {
        unsafe { std::ptr::copy_nonoverlapping(notes.as_ptr(), output, notes.len()) };
    }
    notes.len()
}

/// Hands the published note list to the caller in a buffer this side owns so
/// no caller-specified capacity can size a foreign resize. Returns null, and
/// leaves `count` at zero, for an empty list or an unknown store.
///
/// # Safety
/// `count` must be null or writable for one `usize`.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_notes_snapshot_take(
    store: *const c_void,
    count: *mut usize,
) -> *mut ScheduledMidiNoteFfi {
    if let Some(count) = unsafe { count.as_mut() } {
        *count = 0;
    }
    let Some(store) = (unsafe { store.cast::<MidiSnapshotStore>().as_ref() }) else {
        return std::ptr::null_mut();
    };
    let notes = store.snapshot_notes();
    if notes.is_empty() {
        return std::ptr::null_mut();
    }
    if let Some(count) = unsafe { count.as_mut() } {
        *count = notes.len();
    }
    Box::into_raw(notes.into_boxed_slice()).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_notes_snapshot_take_free(
    notes: *mut ScheduledMidiNoteFfi,
    count: usize,
) {
    if notes.is_null() {
        return;
    }
    unsafe { drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(notes, count))) };
}

/// Replaces the scheduled note list, rejecting a null store or more than
/// 100,000 notes, and records the "Replace MIDI Notes" history entry when
/// asked.
///
/// # Safety
/// `notes` must be readable for `count` entries.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_notes_replace(
    store: *mut c_void,
    notes: *const ScheduledMidiNoteFfi,
    count: usize,
    record_undo: bool,
    undo: *mut c_void,
    log: *const c_void,
) -> bool {
    let Some(store) = (unsafe { store_ref(store) }) else {
        return false;
    };
    if count > 100_000 || (count > 0 && notes.is_null()) {
        return false;
    }
    let notes = if count == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(notes, count) }.to_vec()
    };
    replace_notes(store, notes, record_undo, undo, log)
}

/// Applies a range edit (0=remove, 1=transpose, 2=move) to the published list
/// and republishes it with one history entry. Returns false when the edit is
/// rejected or the publication fails.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_notes_edit_range(
    store: *mut c_void,
    track_id: u32,
    start_sample: u64,
    end_sample: u64,
    operation: u32,
    amount: i64,
    undo: *mut c_void,
    log: *const c_void,
) -> bool {
    let Some(store) = (unsafe { store_ref(store) }) else {
        return false;
    };
    let mut next = store.snapshot_notes();
    let edited = unsafe {
        hirari_midi_edit_range(
            next.as_mut_ptr(),
            next.len(),
            track_id,
            start_sample,
            end_sample,
            operation,
            amount,
        )
    };
    if edited < 0 {
        return false;
    }
    next.truncate(edited as usize);
    replace_notes(store, next, true, undo, log)
}

/// Inserts or updates a legacy track-scoped note and republishes the list with
/// one history entry. Invalid input and an unchanged note leave it untouched.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_notes_upsert(
    store: *mut c_void,
    track_id: u32,
    pitch: u8,
    velocity: u8,
    start_sample: u64,
    length_samples: u64,
    undo: *mut c_void,
    log: *const c_void,
) {
    let Some(store) = (unsafe { store_ref(store) }) else {
        return;
    };
    let mut next = store.snapshot_notes();
    let count = next.len();
    next.push(ScheduledMidiNoteFfi::default());
    let edited = unsafe {
        hirari_midi_note_upsert(
            next.as_mut_ptr(),
            count,
            next.len(),
            track_id,
            pitch,
            velocity,
            start_sample,
            length_samples,
        )
    };
    if edited < 0 {
        return;
    }
    next.truncate(edited as usize);
    replace_notes(store, next, true, undo, log);
}

const PACKED_NOTE_FIELDS: usize = 9;
const MAX_PACKED_NOTES: usize = 100_000;

fn decode_packed_note(fields: &[u64]) -> Option<ScheduledMidiNoteFfi> {
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

fn packed_notes(packed: &[u64]) -> Option<impl Iterator<Item = &[u64]>> {
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

#[cfg(test)]
mod packed_note_tests {
    use super::*;

    fn valid() -> Vec<u64> {
        vec![3, 60, 100, 2, 960, 480, 100, 7, 4]
    }

    fn unpack(packed: &[u64]) -> Result<Vec<ScheduledMidiNoteFfi>, usize> {
        let mut notes = vec![ScheduledMidiNoteFfi::default(); packed.len() / 9 + 1];
        let count = unsafe {
            hirari_midi_notes_unpack(
                packed.as_ptr(),
                packed.len(),
                notes.as_mut_ptr(),
                notes.len(),
            )
        };
        if count == usize::MAX {
            return Err(count);
        }
        notes.truncate(count);
        Ok(notes)
    }

    #[test]
    fn decodes_every_field() {
        let notes = unpack(&valid()).expect("valid vector");
        assert_eq!(
            notes,
            vec![ScheduledMidiNoteFfi {
                track_id: 3,
                pitch: 60,
                velocity: 100,
                midi_channel: 2,
                articulation_id: 4,
                start_sample: 960,
                length_samples: 480,
                probability: 100,
                region_id: 7,
            }]
        );
    }

    #[test]
    fn rejects_a_vector_that_is_not_a_whole_number_of_notes() {
        assert!(unpack(&[1, 2, 3]).is_err());
    }

    #[test]
    fn rejects_out_of_range_fields() {
        for (index, value) in [
            (0, u64::from(u32::MAX) + 1),
            (1, 128),
            (2, 0),
            (2, 128),
            (3, 16),
            (5, 0),
            (6, 101),
            (7, u64::from(u32::MAX) + 1),
            (8, 128),
            (4, u64::MAX),
        ] {
            let mut packed = valid();
            packed[index] = value;
            assert!(unpack(&packed).is_err(), "field {index}={value} must fail");
        }
    }

    #[test]
    fn accepts_the_edge_of_every_range() {
        let mut packed = valid();
        packed[0] = u64::from(u32::MAX);
        packed[1] = 127;
        packed[2] = 1;
        packed[3] = 15;
        packed[4] = u64::MAX - 1;
        packed[5] = 1;
        packed[6] = 0;
        packed[7] = u64::from(u32::MAX);
        packed[8] = 127;
        assert_eq!(unpack(&packed).expect("edge values are valid").len(), 1);
    }

    #[test]
    fn count_request_does_not_write_when_capacity_is_short() {
        let packed = valid();
        let mut sentinel = [ScheduledMidiNoteFfi::default()];
        sentinel[0].pitch = 9;
        let count = unsafe {
            hirari_midi_notes_unpack(packed.as_ptr(), packed.len(), sentinel.as_mut_ptr(), 0)
        };
        assert_eq!(count, 1);
        assert_eq!(sentinel[0].pitch, 9);
    }

    #[test]
    fn packing_round_trips_the_decoded_note() {
        let notes = unpack(&valid()).expect("valid vector");
        let required = unsafe {
            hirari_midi_notes_pack(notes.as_ptr(), notes.len(), std::ptr::null_mut(), 0)
        };
        assert_eq!(required, valid().len());
        let mut packed = vec![0u64; required];
        assert_eq!(
            unsafe {
                hirari_midi_notes_pack(notes.as_ptr(), notes.len(), packed.as_mut_ptr(), packed.len())
            },
            required
        );
        assert_eq!(packed, valid());
    }

    #[test]
    fn packing_needs_room_for_every_field() {
        let notes = unpack(&valid()).expect("valid vector");
        let mut short = [7u64; 8];
        assert_eq!(
            unsafe {
                hirari_midi_notes_pack(notes.as_ptr(), notes.len(), short.as_mut_ptr(), short.len())
            },
            valid().len()
        );
        assert_eq!(short, [7u64; 8]);
    }

    #[test]
    fn empty_vector_reports_zero() {
        let count =
            unsafe { hirari_midi_notes_unpack(std::ptr::null(), 0, std::ptr::null_mut(), 0) };
        assert_eq!(count, 0);
    }
}

#[cfg(test)]
mod edit_host_tests {
    use super::*;
    use crate::undo_transaction::{
        hirari_undo_manager_apply_and_invoke, hirari_undo_manager_create,
        hirari_undo_manager_destroy, hirari_undo_manager_undo_count,
    };

    fn note(track_id: u32, pitch: u8, start_sample: u64) -> ScheduledMidiNoteFfi {
        ScheduledMidiNoteFfi {
            track_id,
            pitch,
            velocity: 100,
            midi_channel: 0,
            articulation_id: 0,
            start_sample,
            length_samples: 480,
            probability: 100,
            region_id: 0,
        }
    }

    fn published(store: *mut c_void) -> Vec<ScheduledMidiNoteFfi> {
        let count = unsafe { hirari_midi_notes_snapshot_copy(store, std::ptr::null_mut(), 0) };
        let mut notes = vec![ScheduledMidiNoteFfi::default(); count];
        let copied =
            unsafe { hirari_midi_notes_snapshot_copy(store, notes.as_mut_ptr(), notes.len()) };
        notes.truncate(copied);
        notes
    }

    #[test]
    fn snapshot_take_hands_the_published_list_over_once() {
        let store = hirari_midi_snapshot_store_create();
        let notes = [note(1, 60, 0), note(2, 64, 960)];
        unsafe {
            assert!(hirari_midi_notes_replace(
                store,
                notes.as_ptr(),
                notes.len(),
                false,
                std::ptr::null_mut(),
                std::ptr::null()
            ));
        }
        let mut count = 0usize;
        let taken = unsafe { hirari_midi_notes_snapshot_take(store, &mut count) };
        assert!(!taken.is_null());
        assert_eq!(count, 2);
        assert_eq!(unsafe { std::slice::from_raw_parts(taken, count) }, &notes);
        unsafe { hirari_midi_notes_snapshot_take_free(taken, count) };
        let mut again = 9usize;
        assert!(unsafe { hirari_midi_notes_snapshot_take(std::ptr::null(), &mut again) }.is_null());
        assert_eq!(again, 0);
        unsafe { hirari_midi_snapshot_store_destroy(store) };
    }

    #[test]
    fn replace_publishes_and_undo_replays_the_previous_list() {
        let store = hirari_midi_snapshot_store_create();
        let undo = hirari_undo_manager_create(8, 300);
        let first = [note(1, 60, 0), note(1, 64, 480)];
        let second = [note(1, 60, 0), note(1, 64, 480), note(2, 67, 960)];
        unsafe {
            assert!(hirari_midi_notes_replace(
                store,
                first.as_ptr(),
                first.len(),
                false,
                std::ptr::null_mut(),
                std::ptr::null()
            ));
            assert_eq!(published(store), first);
            assert!(hirari_midi_notes_replace(
                store,
                second.as_ptr(),
                second.len(),
                true,
                undo,
                std::ptr::null()
            ));
            assert_eq!(published(store), second);
            assert!(hirari_undo_manager_apply_and_invoke(undo, false));
            assert_eq!(published(store), first);
            assert!(hirari_undo_manager_apply_and_invoke(undo, true));
            assert_eq!(published(store), second);
            hirari_undo_manager_destroy(undo);
            hirari_midi_snapshot_store_destroy(store);
        }
    }

    #[test]
    fn range_removal_and_upsert_each_publish_one_history_entry() {
        let store = hirari_midi_snapshot_store_create();
        let undo = hirari_undo_manager_create(8, 300);
        let base = [note(1, 60, 0), note(1, 62, 1000), note(2, 64, 2000)];
        unsafe {
            assert!(hirari_midi_notes_replace(
                store,
                base.as_ptr(),
                base.len(),
                true,
                undo,
                std::ptr::null()
            ));
            assert!(hirari_midi_notes_edit_range(
                store,
                1,
                0,
                500,
                0,
                0,
                undo,
                std::ptr::null()
            ));
            assert_eq!(published(store), [note(1, 62, 1000), note(2, 64, 2000)]);
            hirari_midi_notes_upsert(store, 1, 72, 100, 4000, 480, undo, std::ptr::null());
            assert_eq!(published(store).len(), 3);
            // An identical upsert leaves both the list and the history alone.
            hirari_midi_notes_upsert(store, 1, 72, 100, 4000, 480, undo, std::ptr::null());
            assert_eq!(hirari_undo_manager_undo_count(undo), 3);
            assert!(hirari_undo_manager_apply_and_invoke(undo, false));
            assert_eq!(published(store).len(), 2);
            assert!(hirari_undo_manager_apply_and_invoke(undo, false));
            assert_eq!(published(store), base);
            hirari_undo_manager_destroy(undo);
            hirari_midi_snapshot_store_destroy(store);
        }
    }
}
