//! MIDI note snapshot store, split out of midi_note_edit.rs. The three-slot
//! lock-free reader protocol lives here; FFI bindings in the parent use it.
use super::*;

#[derive(Default)]
pub(crate) struct MidiSnapshot {
    notes_by_start: Vec<ScheduledMidiNoteFfi>,
    indices_by_end: Vec<usize>,
}

pub(crate) struct MidiSnapshotStore {
    slots: [UnsafeCell<MidiSnapshot>; 3],
    active: AtomicUsize,
    readers: [AtomicUsize; 3],
    pub(crate) revision: AtomicU64,
    writer: Mutex<()>,
    // Serializes the read-modify-publish sequence of one note edit, as the
    // engine's control-side mutex did. Plain snapshots do not take it.
    pub(crate) edit: Mutex<()>,
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
    pub(crate) fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| UnsafeCell::new(MidiSnapshot::default())),
            active: AtomicUsize::new(0),
            readers: std::array::from_fn(|_| AtomicUsize::new(0)),
            revision: AtomicU64::new(0),
            writer: Mutex::new(()),
            edit: Mutex::new(()),
        }
    }

    pub(crate) fn publish(&self, snapshot: MidiSnapshot) {
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

    pub(crate) fn pin(&self) -> Option<MidiSnapshotView> {
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

    pub(crate) fn release(&self, slot: usize) {
        if let Some(readers) = self.readers.get(slot) {
            readers.fetch_sub(1, Ordering::Release);
        }
    }

    /// Copies the published notes out of the stable slot. The engine's control
    /// path waited for a pin rather than failing, so this retries the same way.
    pub(crate) fn snapshot_notes(&self) -> Vec<ScheduledMidiNoteFfi> {
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
pub(crate) fn publish_snapshot(store: &MidiSnapshotStore, notes: &[ScheduledMidiNoteFfi]) -> bool {
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
pub(crate) fn replay_snapshot(store: *const MidiSnapshotStore, notes: &[ScheduledMidiNoteFfi]) {
    let _edit = unsafe { &*store }
        .edit
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _ = unsafe { publish_snapshot(&*store, notes) };
}
