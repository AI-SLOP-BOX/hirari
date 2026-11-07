//! Storage for the engine's two lock-free queues: control commands flowing to
//! the audio callback and engine events flowing back to the control thread.
//!
//! Both are single-producer/single-consumer rings. The indices move with
//! release/acquire ordering exactly like the C++ template they replace, so a
//! callback never observes a partially written record. Each engine owns one
//! handle for both rings, so offline engines and tests never share queue state.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU32, Ordering};

pub const COMMAND_RING_CAPACITY: u32 = 1024;
pub const EVENT_RING_CAPACITY: u32 = 512;
pub const EVENT_LABEL_BYTES: usize = 128;
const EVENT_LABEL_MAX: usize = EVENT_LABEL_BYTES - 1;

/// Mirrors the C++ `HirariEngineCommandRecord` byte for byte.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CommandRecord {
    pub type_: u32,
    pub tid: u32,
    pub val: f32,
    pub ts: u64,
    pub expected_audio_generation: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EventRecord {
    pub timestamp: u64,
    pub event_type: u32,
    pub track_id: u32,
    pub value: f32,
    pub label: [u8; EVENT_LABEL_BYTES],
}

impl Default for EventRecord {
    fn default() -> Self {
        Self {
            timestamp: 0,
            event_type: 0,
            track_id: 0,
            value: 0.0,
            label: [0; EVENT_LABEL_BYTES],
        }
    }
}

const EMPTY_COMMAND: CommandRecord = CommandRecord {
    type_: 0,
    tid: 0,
    val: 0.0,
    ts: 0,
    expected_audio_generation: 0,
};
const EMPTY_EVENT: EventRecord = EventRecord {
    timestamp: 0,
    event_type: 0,
    track_id: 0,
    value: 0.0,
    label: [0; EVENT_LABEL_BYTES],
};

macro_rules! spsc_ring {
    ($name:ident, $record:ty, $empty:expr, $capacity:expr) => {
        struct $name {
            slots: UnsafeCell<[$record; $capacity as usize]>,
            write: AtomicU32,
            read: AtomicU32,
        }

        // The ring is only touched through its atomic indices; the producer and
        // consumer never write the same slot.
        unsafe impl Sync for $name {}

        impl $name {
            const fn new() -> Self {
                Self {
                    slots: UnsafeCell::new([$empty; $capacity as usize]),
                    write: AtomicU32::new(0),
                    read: AtomicU32::new(0),
                }
            }

            fn push(&self, value: $record) -> bool {
                let write = self.write.load(Ordering::Relaxed);
                let read = self.read.load(Ordering::Acquire);
                if write.wrapping_sub(read) >= $capacity {
                    return false;
                }
                // SAFETY: the producer owns the slot at `write`; a full check
                // above guarantees it is not the consumer's current slot.
                unsafe {
                    (*self.slots.get())[(write as usize) & ($capacity as usize - 1)] = value;
                }
                self.write.store(write.wrapping_add(1), Ordering::Release);
                true
            }

            fn pop(&self) -> Option<$record> {
                let read = self.read.load(Ordering::Relaxed);
                let write = self.write.load(Ordering::Acquire);
                if read == write {
                    return None;
                }
                // SAFETY: a non-empty ring means the producer published the
                // slot at `read` before advancing `write`.
                let value =
                    unsafe { (*self.slots.get())[(read as usize) & ($capacity as usize - 1)] };
                self.read.store(read.wrapping_add(1), Ordering::Release);
                Some(value)
            }

            // Only valid where producer and consumer are both stopped (project
            // load/new-project): stale commands from the previous document must
            // not reach the first block of the next one.
            fn clear(&self) {
                let write = self.write.load(Ordering::Acquire);
                self.read.store(write, Ordering::Release);
            }
        }
    };
}

spsc_ring!(CommandRing, CommandRecord, EMPTY_COMMAND, COMMAND_RING_CAPACITY);
spsc_ring!(EventRing, EventRecord, EMPTY_EVENT, EVENT_RING_CAPACITY);

/// One engine's pair of queues. C++ holds the handle as an opaque `void*`, so
/// the queue storage and its index protocol stay entirely in Rust.
pub struct EngineRings {
    command: CommandRing,
    event: EventRing,
}

impl EngineRings {
    fn new() -> Self {
        Self {
            command: CommandRing::new(),
            event: EventRing::new(),
        }
    }
}

/// Allocates the rings for one engine. The caller owns the handle and must
/// release it with `hirari_engine_rings_destroy`.
#[no_mangle]
pub extern "C" fn hirari_engine_rings_create() -> *mut EngineRings {
    Box::into_raw(Box::new(EngineRings::new()))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_engine_rings_destroy(rings: *mut EngineRings) {
    if !rings.is_null() {
        // SAFETY: the handle came from `hirari_engine_rings_create` and is only
        // released once, after the engine has joined its audio callbacks.
        drop(unsafe { Box::from_raw(rings) });
    }
}

/// Enqueues one admitted command. Returns false for a full ring or no engine.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_command_ring_push(
    rings: *const EngineRings,
    record: *const CommandRecord,
) -> bool {
    let (Some(rings), Some(record)) = (rings.as_ref(), record.as_ref()) else {
        return false;
    };
    rings.command.push(*record)
}

/// Dequeues one command into `record`. Returns false when the ring is empty.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_command_ring_pop(
    rings: *const EngineRings,
    record: *mut CommandRecord,
) -> bool {
    let (Some(rings), Some(record)) = (rings.as_ref(), record.as_mut()) else {
        return false;
    };
    match rings.command.pop() {
        Some(value) => {
            *record = value;
            true
        }
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_engine_command_ring_clear(rings: *const EngineRings) {
    if let Some(rings) = rings.as_ref() {
        rings.command.clear();
    }
}

/// Enqueues an engine event. The label is truncated at a NUL byte or at
/// `EVENT_LABEL_MAX` bytes, matching the previous snprintf truncation.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_event_ring_push(
    rings: *const EngineRings,
    timestamp: u64,
    event_type: u32,
    track_id: u32,
    value: f32,
    label: *const u8,
    label_len: usize,
) -> bool {
    let Some(rings) = rings.as_ref() else {
        return false;
    };
    let mut record = EventRecord {
        timestamp,
        event_type,
        track_id,
        value,
        ..EventRecord::default()
    };
    if label_len > 0 {
        if label.is_null() {
            return false;
        }
        // SAFETY: the caller passes a readable buffer of at least `label_len`.
        let source = unsafe { std::slice::from_raw_parts(label, label_len.min(EVENT_LABEL_MAX)) };
        let length = source
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(source.len());
        record.label[..length].copy_from_slice(&source[..length]);
    }
    rings.event.push(record)
}

/// Dequeues one engine event. The label copy is always NUL-terminated inside
/// `label_capacity`, mirroring the old fixed-buffer fill.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_event_ring_pop(
    rings: *const EngineRings,
    timestamp: *mut u64,
    event_type: *mut u32,
    track_id: *mut u32,
    value: *mut f32,
    label: *mut u8,
    label_capacity: usize,
) -> bool {
    if timestamp.is_null()
        || event_type.is_null()
        || track_id.is_null()
        || value.is_null()
        || label.is_null()
        || label_capacity == 0
    {
        return false;
    }
    let Some(rings) = rings.as_ref() else {
        return false;
    };
    let Some(record) = rings.event.pop() else {
        return false;
    };
    let copy = label_capacity.min(EVENT_LABEL_BYTES);
    // SAFETY: every out-pointer was checked non-null and `copy` is bounded by
    // both the destination capacity and the fixed label width.
    unsafe {
        *timestamp = record.timestamp;
        *event_type = record.event_type;
        *track_id = record.track_id;
        *value = record.value;
        std::ptr::copy_nonoverlapping(record.label.as_ptr(), label, copy);
        *label.add(copy - 1) = 0;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle() -> *mut EngineRings {
        hirari_engine_rings_create()
    }

    fn command(tid: u32) -> CommandRecord {
        CommandRecord {
            type_: 1,
            tid,
            val: 0.5,
            ts: 42,
            expected_audio_generation: 7,
        }
    }

    #[test]
    fn command_ring_is_fifo_and_preserves_generation() {
        let rings = handle();
        assert!(unsafe { hirari_engine_command_ring_push(rings, &command(3)) });
        assert!(unsafe { hirari_engine_command_ring_push(rings, &command(9)) });
        let mut out = CommandRecord::default();
        assert!(unsafe { hirari_engine_command_ring_pop(rings, &mut out) });
        assert_eq!(out, command(3));
        assert_eq!(out.expected_audio_generation, 7);
        assert!(unsafe { hirari_engine_command_ring_pop(rings, &mut out) });
        assert_eq!(out.tid, 9);
        assert!(!unsafe { hirari_engine_command_ring_pop(rings, &mut out) });
        unsafe { hirari_engine_rings_destroy(rings) };
    }

    #[test]
    fn engines_do_not_share_queues() {
        let first = handle();
        let second = handle();
        assert!(unsafe { hirari_engine_command_ring_push(first, &command(1)) });
        let mut out = CommandRecord::default();
        assert!(!unsafe { hirari_engine_command_ring_pop(second, &mut out) });
        assert!(unsafe { hirari_engine_command_ring_pop(first, &mut out) });
        assert_eq!(out.tid, 1);
        unsafe {
            hirari_engine_rings_destroy(first);
            hirari_engine_rings_destroy(second);
        }
    }

    #[test]
    fn command_ring_reports_full_at_capacity() {
        let rings = handle();
        for tid in 0..COMMAND_RING_CAPACITY {
            assert!(unsafe { hirari_engine_command_ring_push(rings, &command(tid)) });
        }
        assert!(!unsafe { hirari_engine_command_ring_push(rings, &command(9999)) });
        let mut out = CommandRecord::default();
        assert!(unsafe { hirari_engine_command_ring_pop(rings, &mut out) });
        assert_eq!(out.tid, 0);
        assert!(unsafe { hirari_engine_command_ring_push(rings, &command(9999)) });
        unsafe { hirari_engine_rings_destroy(rings) };
    }

    #[test]
    fn command_ring_clear_discards_pending_records() {
        let rings = handle();
        assert!(unsafe { hirari_engine_command_ring_push(rings, &command(1)) });
        unsafe { hirari_engine_command_ring_clear(rings) };
        let mut out = CommandRecord::default();
        assert!(!unsafe { hirari_engine_command_ring_pop(rings, &mut out) });
        unsafe { hirari_engine_rings_destroy(rings) };
    }

    #[test]
    fn null_handles_and_records_are_rejected() {
        let rings = handle();
        let null = std::ptr::null::<EngineRings>();
        let mut out = CommandRecord::default();
        assert!(!unsafe { hirari_engine_command_ring_push(null, &command(1)) });
        assert!(!unsafe { hirari_engine_command_ring_push(rings, std::ptr::null()) });
        assert!(!unsafe { hirari_engine_command_ring_pop(null, &mut out) });
        assert!(!unsafe { hirari_engine_command_ring_pop(rings, std::ptr::null_mut()) });
        assert!(
            !unsafe { hirari_engine_event_ring_push(null, 0, 0, 0, 0.0, std::ptr::null(), 0) }
        );
        unsafe {
            hirari_engine_command_ring_clear(null);
            hirari_engine_rings_destroy(std::ptr::null_mut());
            hirari_engine_rings_destroy(rings);
        }
    }

    #[test]
    fn event_ring_round_trips_and_truncates_labels() {
        let rings = handle();
        let label = b"clip\twarped";
        assert!(unsafe {
            hirari_engine_event_ring_push(rings, 11, 0x50, 4, 1.25, label.as_ptr(), label.len())
        });
        let (mut timestamp, mut event_type, mut track_id, mut value) = (0, 0, 0, 0.0);
        let mut out = [0xAAu8; EVENT_LABEL_BYTES];
        assert!(unsafe {
            hirari_engine_event_ring_pop(
                rings,
                &mut timestamp,
                &mut event_type,
                &mut track_id,
                &mut value,
                out.as_mut_ptr(),
                out.len(),
            )
        });
        assert_eq!((timestamp, event_type, track_id), (11, 0x50, 4));
        assert_eq!(value, 1.25);
        assert_eq!(&out[..label.len()], label);
        assert_eq!(out[label.len()], 0);
        unsafe { hirari_engine_rings_destroy(rings) };
    }

    #[test]
    fn event_label_is_cut_at_nul_and_ring_capacity() {
        let rings = handle();
        let label = b"abc\0def";
        assert!(unsafe {
            hirari_engine_event_ring_push(rings, 0, 1, 1, 0.0, label.as_ptr(), label.len())
        });
        let long = [b'z'; EVENT_LABEL_BYTES + 16];
        assert!(unsafe {
            hirari_engine_event_ring_push(rings, 0, 2, 2, 0.0, long.as_ptr(), long.len())
        });
        let mut out = [0xAAu8; EVENT_LABEL_BYTES];
        let (mut t, mut e, mut id, mut v) = (0, 0, 0, 0.0);
        assert!(unsafe {
            hirari_engine_event_ring_pop(
                rings,
                &mut t,
                &mut e,
                &mut id,
                &mut v,
                out.as_mut_ptr(),
                out.len(),
            )
        });
        assert_eq!(&out[..5], b"abc\0\0");
        assert!(unsafe {
            hirari_engine_event_ring_pop(
                rings,
                &mut t,
                &mut e,
                &mut id,
                &mut v,
                out.as_mut_ptr(),
                out.len(),
            )
        });
        assert_eq!(&out[..EVENT_LABEL_MAX], &long[..EVENT_LABEL_MAX]);
        assert_eq!(out[EVENT_LABEL_MAX], 0);
        assert!(!unsafe {
            hirari_engine_event_ring_pop(rings, &mut t, &mut e, &mut id, &mut v, out.as_mut_ptr(), 0)
        });
        unsafe { hirari_engine_rings_destroy(rings) };
    }
}
