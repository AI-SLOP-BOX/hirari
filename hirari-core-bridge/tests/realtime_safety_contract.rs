//! The realtime-safety contract, driven through the same entry points the
//! engine's block head uses. This test used to be a C++ translation unit
//! (`tests/realtime_safety_contract.cpp`) that reached these stores over the
//! flat C ABI; the Rust owner can pin the same guarantees from its own side of
//! that boundary.

use hirari_core_bridge::audio_buffer_ops::{
    hirari_audio_buffer_last_realtime_requested_capacity,
    hirari_audio_buffer_realtime_resize_attempts, hirari_audio_buffer_set_realtime_guard,
    hirari_audio_buffer_storage_create, hirari_audio_buffer_storage_destroy,
    hirari_audio_buffer_storage_reserve_buffer, hirari_audio_buffer_storage_resize,
};
use hirari_core_bridge::midi_buffer_ops::{
    hirari_midi_buffer_add_note_on, hirari_midi_buffer_copy, hirari_midi_buffer_create,
    hirari_midi_buffer_destroy, hirari_midi_buffer_dropped_count, hirari_midi_buffer_event_count,
    hirari_midi_buffer_event_data, hirari_midi_buffer_overflowed, hirari_midi_buffer_take_dropped,
    hirari_midi_buffer_take_overflowed, MIDI_EVENT_CAPACITY,
};
use hirari_core_bridge::status_queue::{
    hirari_status_queue_pop_default, StatusMessage, SEVERITY_CRITICAL,
};

/// The event layout `hirari_midi_buffer_copy` reads, byte-for-byte the shape the
/// deleted C++ contract built on the stack.
#[repr(C)]
struct FfiMidiEvent {
    sample_offset: u64,
    size: u32,
    data: [u8; 256],
    articulation_id: u8,
}

fn drain_notifications() {
    let mut message = StatusMessage::default();
    loop {
        if !unsafe { hirari_status_queue_pop_default(&mut message) } {
            return;
        }
    }
}

fn take_notification() -> Option<StatusMessage> {
    let mut message = StatusMessage::default();
    unsafe { hirari_status_queue_pop_default(&mut message) }.then_some(message)
}

fn text_of(message: &StatusMessage) -> String {
    let end = message
        .text
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(message.text.len());
    String::from_utf8_lossy(&message.text[..end]).into_owned()
}

#[test]
fn a_full_event_table_reports_the_drop_and_lets_the_callback_take_it() {
    let midi = hirari_midi_buffer_create();
    assert_ne!(midi, std::ptr::null_mut());
    for index in 0..MIDI_EVENT_CAPACITY as u64 {
        unsafe { hirari_midi_buffer_add_note_on(midi, 1, 60, 100, index, 0) };
    }
    unsafe { hirari_midi_buffer_add_note_on(midi, 1, 61, 100, MIDI_EVENT_CAPACITY as u64, 0) };

    assert_eq!(
        unsafe { hirari_midi_buffer_event_count(midi) },
        MIDI_EVENT_CAPACITY
    );
    assert!(unsafe { hirari_midi_buffer_overflowed(midi) });
    assert_eq!(unsafe { hirari_midi_buffer_dropped_count(midi) }, 1);
    // The flag and the counter are taken, not read: the callback reports each
    // overflow once, so the next block starts clean.
    assert!(unsafe { hirari_midi_buffer_take_overflowed(midi) });
    assert_eq!(unsafe { hirari_midi_buffer_take_dropped(midi) }, 1);
    assert!(!unsafe { hirari_midi_buffer_overflowed(midi) });

    unsafe { hirari_midi_buffer_destroy(midi) };
}

#[test]
fn an_empty_payload_still_occupies_one_event_slot() {
    let buffer = hirari_midi_buffer_create();
    let empty = FfiMidiEvent {
        sample_offset: 0,
        size: 0,
        data: [0; 256],
        articulation_id: 0,
    };
    assert!(unsafe { hirari_midi_buffer_copy(buffer, (&empty as *const FfiMidiEvent).cast()) });
    assert_eq!(unsafe { hirari_midi_buffer_event_count(buffer) }, 1);
    let events = unsafe { hirari_midi_buffer_event_data(buffer) }.cast::<FfiMidiEvent>();
    assert!(!events.is_null());
    assert_eq!(unsafe { (*events).data[0] }, 0);
    unsafe { hirari_midi_buffer_destroy(buffer) };
}

#[test]
fn the_realtime_guard_declines_growth_and_names_the_reason() {
    drain_notifications();
    let buffer = hirari_audio_buffer_storage_create();
    assert_eq!(
        unsafe { hirari_audio_buffer_storage_reserve_buffer(buffer, 1, 16) },
        0
    );
    assert!(unsafe { hirari_audio_buffer_storage_resize(buffer, 1, 16) });

    let attempts = hirari_audio_buffer_realtime_resize_attempts();
    hirari_audio_buffer_set_realtime_guard(true);
    let declined = unsafe { hirari_audio_buffer_storage_reserve_buffer(buffer, 2, 64) };
    hirari_audio_buffer_set_realtime_guard(false);

    assert_eq!(declined, 1);
    assert_eq!(hirari_audio_buffer_realtime_resize_attempts(), attempts + 1);
    assert!(hirari_audio_buffer_last_realtime_requested_capacity() >= 2 * 64);

    // The rejection announces itself, because the callback only gets a status
    // code back and cannot report the reason itself.
    let message = take_notification().expect("the declined growth is published");
    assert_eq!(message.severity, SEVERITY_CRITICAL);
    assert_eq!(text_of(&message), "AUDIO_BUFFER_RESIZE_ATTEMPTED");
    drain_notifications();
    unsafe { hirari_audio_buffer_storage_destroy(buffer) };
}
