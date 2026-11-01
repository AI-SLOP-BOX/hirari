use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::midi_note_edit::{
    hirari_midi_schedule_block, hirari_midi_snapshot_store_pin,
    hirari_midi_snapshot_store_release, MidiSnapshotView, ScheduledMidiEventCallback,
};

pub type MidiPanicCallback = unsafe extern "C" fn(*mut c_void);

/// Per-engine MIDI playback bookkeeping. The host callback reports whether
/// playback is active and which block it is about to render; this clock owns
/// the discontinuity/revision comparison that decides when voices must be
/// reconciled, plus the thread-safe panic request raised by transport stops.
struct MidiPlaybackClock {
    previous_block_end: u64,
    previous_snapshot_revision: u64,
    playback_pass: u64,
    has_previous_block: bool,
    previous_playback_active: bool,
    panic_requested: AtomicBool,
}

impl MidiPlaybackClock {
    fn new() -> Self {
        Self {
            previous_block_end: 0,
            previous_snapshot_revision: 0,
            playback_pass: 0,
            has_previous_block: false,
            previous_playback_active: false,
            panic_requested: AtomicBool::new(false),
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_midi_playback_clock_create() -> *mut c_void {
    Box::into_raw(Box::new(MidiPlaybackClock::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_playback_clock_destroy(clock: *mut c_void) {
    if !clock.is_null() {
        drop(unsafe { Box::from_raw(clock.cast::<MidiPlaybackClock>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_playback_request_panic(clock: *mut c_void) {
    if let Some(clock) = unsafe { clock.cast::<MidiPlaybackClock>().as_ref() } {
        clock.panic_requested.store(true, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_playback_panic_pending(clock: *const c_void) -> bool {
    unsafe { clock.cast::<MidiPlaybackClock>().as_ref() }
        .is_some_and(|clock| clock.panic_requested.load(Ordering::Acquire))
}

/// Runs one block of the MIDI playback state machine: pins the published
/// note snapshot, compares it against the previous block, requests a voice
/// reconciliation when playback jumped or the notes changed, and dispatches
/// the scheduled notes through `note_callback`. The host owns the voices, so
/// `panic_callback` performs the actual all-notes-off/reset.
///
/// Returns bit 0 when a snapshot was pinned and bit 1 when voices were
/// reconciled.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_playback_prepare_block(
    clock: *mut c_void,
    store: *const c_void,
    playhead: u64,
    frames: u32,
    playback_active: u8,
    context: *mut c_void,
    panic_callback: Option<MidiPanicCallback>,
    note_callback: Option<ScheduledMidiEventCallback>,
) -> u8 {
    let Some(clock) = (unsafe { clock.cast::<MidiPlaybackClock>().as_mut() }) else {
        return 0;
    };
    let Some(note_callback) = note_callback else {
        return 0;
    };
    let Some(block_end) = playhead.checked_add(u64::from(frames)) else {
        return 0;
    };
    let active = playback_active != 0;

    let mut snapshot = MidiSnapshotView::default();
    let pinned = unsafe { hirari_midi_snapshot_store_pin(store, &mut snapshot) };
    let revision = snapshot.revision;

    let discontinuity = active
        && (!clock.has_previous_block
            || !clock.previous_playback_active
            || playhead != clock.previous_block_end);
    if discontinuity {
        clock.playback_pass += 1;
    }
    let snapshot_changed = pinned
        && clock.has_previous_block
        && revision != clock.previous_snapshot_revision;
    let requested_panic = clock.panic_requested.swap(false, Ordering::AcqRel);
    let reconcile = discontinuity || snapshot_changed || requested_panic || !pinned;
    if reconcile {
        if let Some(panic) = panic_callback {
            unsafe { panic(context) };
        }
    }

    if pinned {
        unsafe {
            hirari_midi_schedule_block(
                snapshot.notes,
                snapshot.note_count,
                snapshot.indices_by_end,
                snapshot.end_count,
                playhead,
                frames,
                active as u8,
                reconcile as u8,
                clock.playback_pass,
                context,
                Some(note_callback),
            );
            hirari_midi_snapshot_store_release(store, snapshot.slot);
        }
    } else {
        // Under sustained concurrent edits, never risk traversing a mutable
        // snapshot. Panic this block and reconcile on the next callback after
        // a stable slot can be pinned.
        clock.panic_requested.store(true, Ordering::Release);
    }

    clock.previous_playback_active = pinned && active;
    clock.previous_block_end = block_end;
    if pinned {
        clock.previous_snapshot_revision = revision;
    }
    clock.has_previous_block = true;
    (pinned as u8) | ((reconcile as u8) << 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi_note_edit::{
        hirari_midi_snapshot_store_create, hirari_midi_snapshot_store_destroy,
        hirari_midi_snapshot_store_publish, ScheduledMidiNoteFfi,
    };
    use std::cell::RefCell;

    #[derive(Default)]
    struct Recorded {
        panics: u32,
        ons: Vec<(u64, u8)>,
        offsets: Vec<u64>,
    }

    thread_local! {
        static RECORDED: RefCell<Recorded> = RefCell::new(Recorded::default());
    }

    unsafe extern "C" fn panic(_: *mut c_void) {
        RECORDED.with(|recorded| recorded.borrow_mut().panics += 1);
    }

    unsafe extern "C" fn note(
        _: *mut c_void,
        note: *const ScheduledMidiNoteFfi,
        offset: u64,
        note_off: u8,
    ) {
        RECORDED.with(|recorded| {
            let mut recorded = recorded.borrow_mut();
            if note_off == 0 {
                recorded.ons.push((offset, unsafe { (*note).pitch }));
            }
            recorded.offsets.push(offset);
        });
    }

    fn take_recorded() -> Recorded {
        RECORDED.with(|recorded| std::mem::take(&mut *recorded.borrow_mut()))
    }

    struct Fixture {
        store: *mut c_void,
        clock: *mut c_void,
    }

    impl Fixture {
        fn new(notes: &[ScheduledMidiNoteFfi]) -> Self {
            let store = hirari_midi_snapshot_store_create();
            assert!(unsafe { hirari_midi_snapshot_store_publish(store, notes.as_ptr(), notes.len()) });
            let clock = hirari_midi_playback_clock_create();
            Self { store, clock }
        }

        fn block(&self, playhead: u64, active: bool) -> u8 {
            unsafe {
                hirari_midi_playback_prepare_block(
                    self.clock,
                    self.store,
                    playhead,
                    512,
                    active as u8,
                    std::ptr::null_mut(),
                    Some(panic),
                    Some(note),
                )
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe {
                hirari_midi_playback_clock_destroy(self.clock);
                hirari_midi_snapshot_store_destroy(self.store);
            }
        }
    }

    fn note_at(pitch: u8, start: u64, length: u64) -> ScheduledMidiNoteFfi {
        ScheduledMidiNoteFfi {
            track_id: 7,
            pitch,
            velocity: 100,
            midi_channel: 0,
            articulation_id: 0,
            start_sample: start,
            length_samples: length,
            probability: 100,
            region_id: 0,
        }
    }

    #[test]
    fn first_active_block_reconciles_and_dispatches() {
        let _ = take_recorded();
        let fixture = Fixture::new(&[note_at(60, 0, 256)]);
        let flags = fixture.block(0, true);
        assert_eq!(flags & 1, 1);
        assert_eq!(flags >> 1, 1);
        let recorded = take_recorded();
        assert_eq!(recorded.panics, 1);
        assert_eq!(recorded.ons, vec![(0, 60)]);
    }

    #[test]
    fn continuous_blocks_do_not_reconcile_or_advance_the_pass() {
        let _ = take_recorded();
        let fixture = Fixture::new(&[note_at(60, 0, 4096)]);
        assert_eq!(fixture.block(0, true), 3);
        let _ = take_recorded();
        // Same playhead progression, same snapshot: no panic, no new note-on.
        let flags = fixture.block(512, true);
        assert_eq!(flags, 1);
        let recorded = take_recorded();
        assert_eq!(recorded.panics, 0);
        assert!(recorded.ons.is_empty());
    }

    #[test]
    fn playhead_jump_reconciles_and_replays_active_notes() {
        let _ = take_recorded();
        let fixture = Fixture::new(&[note_at(60, 0, 4096)]);
        assert_eq!(fixture.block(0, true), 3);
        let _ = take_recorded();
        // Discontinuous seek into the middle of the note.
        let flags = fixture.block(1024, true);
        assert_eq!(flags, 3);
        let recorded = take_recorded();
        assert_eq!(recorded.panics, 1);
        assert_eq!(recorded.ons, vec![(0, 60)]);
    }

    #[test]
    fn stopped_transport_pins_without_dispatching_or_reconciling() {
        let _ = take_recorded();
        let fixture = Fixture::new(&[note_at(60, 0, 4096)]);
        let flags = fixture.block(0, false);
        assert_eq!(flags, 1);
        let recorded = take_recorded();
        assert_eq!(recorded.panics, 0);
        assert!(recorded.ons.is_empty());
    }

    #[test]
    fn pending_panic_is_consumed_by_the_next_block() {
        let fixture = Fixture::new(&[note_at(60, 0, 4096)]);
        let _ = fixture.block(0, true);
        let _ = take_recorded();
        unsafe { hirari_midi_playback_request_panic(fixture.clock) };
        assert!(unsafe { hirari_midi_playback_panic_pending(fixture.clock) });
        assert_eq!(fixture.block(512, true) >> 1, 1);
        assert!(!unsafe { hirari_midi_playback_panic_pending(fixture.clock) });
        assert_eq!(take_recorded().panics, 1);
        let _ = take_recorded();
        assert_eq!(fixture.block(1024, true) >> 1, 0);
        assert_eq!(take_recorded().panics, 0);
    }

    #[test]
    fn null_inputs_are_rejected_without_panicking() {
        unsafe {
            assert_eq!(hirari_midi_playback_panic_pending(std::ptr::null()), false);
            hirari_midi_playback_request_panic(std::ptr::null_mut());
            hirari_midi_playback_clock_destroy(std::ptr::null_mut());
            assert_eq!(
                hirari_midi_playback_prepare_block(
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    0,
                    512,
                    1,
                    std::ptr::null_mut(),
                    Some(panic),
                    Some(note),
                ),
                0
            );
        }
    }
}
