use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

const LOG_CAPACITY: usize = 1024;
const LOG_TEXT_BYTES: usize = 96;
const STATE_STORAGE_BYTES: usize = 128 * 1024;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LogEntry {
    pub timestamp: u64,
    pub level: u32,
    pub component_id: u32,
    pub message: [u8; LOG_TEXT_BYTES],
}

impl Default for LogEntry {
    fn default() -> Self {
        Self {
            timestamp: 0,
            level: 0,
            component_id: 0,
            message: [0; LOG_TEXT_BYTES],
        }
    }
}

struct Slot {
    sequence: AtomicU64,
    entry: UnsafeCell<LogEntry>,
}

impl Slot {
    fn new(sequence: u64) -> Self {
        Self {
            sequence: AtomicU64::new(sequence),
            entry: UnsafeCell::new(LogEntry::default()),
        }
    }
}

// A sequence claim grants exclusive access to a slot to exactly one producer
// or consumer until its release publication.
unsafe impl Sync for Slot {}

#[repr(C, align(64))]
struct LogQueue {
    slots: [Slot; LOG_CAPACITY],
    enqueue_position: AtomicU64,
    dequeue_position: AtomicU64,
}

impl LogQueue {
    fn new() -> Self {
        Self {
            slots: std::array::from_fn(|index| Slot::new(index as u64)),
            enqueue_position: AtomicU64::new(0),
            dequeue_position: AtomicU64::new(0),
        }
    }

    fn push(&self, entry: LogEntry) -> bool {
        let mut position = self.enqueue_position.load(Ordering::Relaxed);
        loop {
            let slot = &self.slots[position as usize & (LOG_CAPACITY - 1)];
            let sequence = slot.sequence.load(Ordering::Acquire);
            let difference = sequence.wrapping_sub(position) as i64;
            if difference == 0 {
                match self.enqueue_position.compare_exchange_weak(
                    position,
                    position.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // SAFETY: this producer owns the claimed slot until it
                        // publishes the next sequence number.
                        unsafe { *slot.entry.get() = entry };
                        slot.sequence
                            .store(position.wrapping_add(1), Ordering::Release);
                        return true;
                    }
                    Err(observed) => position = observed,
                }
            } else if difference < 0 {
                return false;
            } else {
                position = self.enqueue_position.load(Ordering::Relaxed);
            }
        }
    }

    fn pop(&self) -> Option<LogEntry> {
        let mut position = self.dequeue_position.load(Ordering::Relaxed);
        loop {
            let slot = &self.slots[position as usize & (LOG_CAPACITY - 1)];
            let sequence = slot.sequence.load(Ordering::Acquire);
            let difference = sequence.wrapping_sub(position.wrapping_add(1)) as i64;
            if difference == 0 {
                match self.dequeue_position.compare_exchange_weak(
                    position,
                    position.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // SAFETY: this consumer owns the published slot until
                        // it advances the sequence by one queue generation.
                        let entry = unsafe { *slot.entry.get() };
                        slot.sequence.store(
                            position.wrapping_add(LOG_CAPACITY as u64),
                            Ordering::Release,
                        );
                        return Some(entry);
                    }
                    Err(observed) => position = observed,
                }
            } else if difference < 0 {
                return None;
            } else {
                position = self.dequeue_position.load(Ordering::Relaxed);
            }
        }
    }
}

const _: () = assert!(std::mem::size_of::<LogQueue>() <= STATE_STORAGE_BYTES);

static CLOCK_ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

#[no_mangle]
pub unsafe extern "C" fn hirari_log_buffer_init(storage: *mut c_void, bytes: usize) -> *mut c_void {
    if storage.is_null()
        || (storage as usize) % std::mem::align_of::<LogQueue>() != 0
        || bytes < std::mem::size_of::<LogQueue>()
    {
        return std::ptr::null_mut();
    }
    let state = storage.cast::<LogQueue>();
    // SAFETY: the caller supplies exclusive aligned storage of sufficient size.
    unsafe { std::ptr::write(state, LogQueue::new()) };
    let _ = CLOCK_ORIGIN.get_or_init(Instant::now);
    state.cast()
}

#[no_mangle]
pub extern "C" fn hirari_log_buffer_timestamp() -> u64 {
    let origin = CLOCK_ORIGIN.get_or_init(Instant::now);
    Instant::now()
        .saturating_duration_since(*origin)
        .as_nanos()
        .min(u64::MAX as u128) as u64
}

#[no_mangle]
pub unsafe extern "C" fn hirari_log_buffer_post(
    state: *const c_void,
    level: u32,
    component_id: u32,
    message: *const u8,
    message_len: usize,
) {
    if state.is_null() || (message_len != 0 && message.is_null()) {
        return;
    }
    let mut entry = LogEntry {
        timestamp: hirari_log_buffer_timestamp(),
        level,
        component_id,
        ..LogEntry::default()
    };
    let length = message_len.min(LOG_TEXT_BYTES - 1);
    if length != 0 {
        // SAFETY: the C++ string_view remains valid for the duration of this call.
        let bytes = unsafe { std::slice::from_raw_parts(message, length) };
        entry.message[..length].copy_from_slice(bytes);
    }
    entry.message[length] = 0;
    // SAFETY: state points to a live queue in caller-owned storage.
    let _ = unsafe { &*state.cast::<LogQueue>() }.push(entry);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_log_buffer_pop(
    state: *const c_void,
    output: *mut LogEntry,
) -> bool {
    if state.is_null() || output.is_null() {
        return false;
    }
    // SAFETY: state is live and queue slots provide exclusive access on pop.
    let Some(entry) = (unsafe { &*state.cast::<LogQueue>() }).pop() else {
        return false;
    };
    // SAFETY: output is a writable LogEntry supplied by the C++ adapter.
    unsafe { output.write(entry) };
    true
}
