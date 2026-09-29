use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const QUEUE_CAPACITY: usize = 256;
const MESSAGE_TEXT_BYTES: usize = 128;
const STATE_STORAGE_BYTES: usize = 48 * 1024;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusMessage {
    pub severity: u32,
    pub text: [u8; MESSAGE_TEXT_BYTES],
}

impl Default for StatusMessage {
    fn default() -> Self {
        Self {
            severity: 0,
            text: [0; MESSAGE_TEXT_BYTES],
        }
    }
}

struct Slot {
    sequence: AtomicUsize,
    value: UnsafeCell<StatusMessage>,
}

impl Slot {
    fn new(sequence: usize) -> Self {
        Self {
            sequence: AtomicUsize::new(sequence),
            value: UnsafeCell::new(StatusMessage::default()),
        }
    }
}

// The sequence number grants exactly one producer or consumer access to value
// at a time; release/acquire publication protects each read and write.
unsafe impl Sync for Slot {}

#[repr(C, align(64))]
struct StatusQueue {
    slots: [Slot; QUEUE_CAPACITY],
    enqueue_position: AtomicUsize,
    dequeue_position: AtomicUsize,
    dropped: AtomicU64,
}

impl StatusQueue {
    fn new() -> Self {
        Self {
            slots: std::array::from_fn(Slot::new),
            enqueue_position: AtomicUsize::new(0),
            dequeue_position: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    fn push(&self, mut message: StatusMessage) -> bool {
        if let Some(terminator) = message.text.iter().position(|byte| *byte == 0) {
            message.text[terminator] = 0;
        } else {
            message.text[MESSAGE_TEXT_BYTES - 1] = 0;
        }
        let mut position = self.enqueue_position.load(Ordering::Relaxed);
        loop {
            let slot = &self.slots[position & (QUEUE_CAPACITY - 1)];
            let sequence = slot.sequence.load(Ordering::Acquire);
            let difference = sequence.wrapping_sub(position) as isize;
            if difference == 0 {
                match self.enqueue_position.compare_exchange_weak(
                    position,
                    position.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // SAFETY: the successful position claim gives this producer
                        // exclusive access to this slot until sequence publication.
                        unsafe { *slot.value.get() = message };
                        slot.sequence
                            .store(position.wrapping_add(1), Ordering::Release);
                        return true;
                    }
                    Err(observed) => position = observed,
                }
            } else if difference < 0 {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return false;
            } else {
                position = self.enqueue_position.load(Ordering::Relaxed);
            }
        }
    }

    fn pop(&self) -> Option<StatusMessage> {
        let mut position = self.dequeue_position.load(Ordering::Relaxed);
        loop {
            let slot = &self.slots[position & (QUEUE_CAPACITY - 1)];
            let sequence = slot.sequence.load(Ordering::Acquire);
            let difference = sequence.wrapping_sub(position.wrapping_add(1)) as isize;
            if difference == 0 {
                match self.dequeue_position.compare_exchange_weak(
                    position,
                    position.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // SAFETY: the successful position claim gives this consumer
                        // exclusive access to the published slot until it is released.
                        let message = unsafe { *slot.value.get() };
                        slot.sequence
                            .store(position.wrapping_add(QUEUE_CAPACITY), Ordering::Release);
                        return Some(message);
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

const _: () = assert!(std::mem::size_of::<StatusQueue>() <= STATE_STORAGE_BYTES);

#[no_mangle]
pub unsafe extern "C" fn hirari_status_queue_init(
    storage: *mut c_void,
    bytes: usize,
) -> *mut c_void {
    if storage.is_null()
        || (storage as usize) % std::mem::align_of::<StatusQueue>() != 0
        || bytes < std::mem::size_of::<StatusQueue>()
    {
        return std::ptr::null_mut();
    }
    let state = storage.cast::<StatusQueue>();
    // SAFETY: The caller supplies suitably aligned, sufficiently large storage
    // that remains alive until the matching destroy call.
    unsafe { std::ptr::write(state, StatusQueue::new()) };
    storage
}

#[no_mangle]
pub unsafe extern "C" fn hirari_status_queue_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: State was initialized in place and is destroyed exactly once.
        unsafe { std::ptr::drop_in_place(state.cast::<StatusQueue>()) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_status_queue_push(
    state: *const c_void,
    severity: u32,
    text: *const u8,
    text_size: usize,
) -> bool {
    let Some(state) = state.cast::<StatusQueue>().as_ref() else {
        return false;
    };
    if text_size != 0 && text.is_null() {
        return false;
    }
    let mut message = StatusMessage {
        severity,
        text: [0; MESSAGE_TEXT_BYTES],
    };
    let copy_size = text_size.min(MESSAGE_TEXT_BYTES - 1);
    if copy_size != 0 {
        // SAFETY: Caller supplies text_size readable bytes; destination is fixed and bounded.
        unsafe { std::ptr::copy_nonoverlapping(text, message.text.as_mut_ptr(), copy_size) };
    }
    state.push(message)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_status_queue_pop(
    state: *const c_void,
    output: *mut StatusMessage,
) -> bool {
    let (Some(state), Some(output)) = (
        state.cast::<StatusQueue>().as_ref(),
        output.cast::<StatusMessage>().as_mut(),
    ) else {
        return false;
    };
    let Some(message) = state.pop() else {
        return false;
    };
    *output = message;
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_status_queue_dropped(state: *const c_void) -> u64 {
    state
        .cast::<StatusQueue>()
        .as_ref()
        .map_or(0, |state| state.dropped.load(Ordering::Relaxed))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_status_queue_take_dropped(state: *const c_void) -> u64 {
    state
        .cast::<StatusQueue>()
        .as_ref()
        .map_or(0, |state| state.dropped.swap(0, Ordering::AcqRel))
}

#[cfg(test)]
mod tests {
    use super::{StatusMessage, StatusQueue, MESSAGE_TEXT_BYTES, QUEUE_CAPACITY};
    use std::sync::Arc;
    use std::thread;

    unsafe extern "C" {
        fn hirari_status_queue_cpp_wrapper_smoke() -> bool;
    }

    #[test]
    fn queue_preserves_severity_truncates_text_and_counts_overflow() {
        let queue = StatusQueue::new();
        let mut message = StatusMessage {
            severity: 3,
            text: [b'x'; MESSAGE_TEXT_BYTES],
        };
        assert!(queue.push(message));
        for _ in 1..QUEUE_CAPACITY {
            assert!(queue.push(StatusMessage::default()));
        }
        assert!(!queue.push(StatusMessage::default()));
        assert_eq!(queue.dropped.load(std::sync::atomic::Ordering::Relaxed), 1);
        let first = queue.pop().unwrap();
        assert_eq!(first.severity, 3);
        assert_eq!(first.text[MESSAGE_TEXT_BYTES - 1], 0);
        message = queue.pop().unwrap();
        assert_eq!(message, StatusMessage::default());
    }

    #[test]
    fn multiple_producers_publish_every_message_once() {
        let queue = Arc::new(StatusQueue::new());
        let producers: Vec<_> = (0..4)
            .map(|producer| {
                let queue = Arc::clone(&queue);
                thread::spawn(move || {
                    for sequence in 0..32u32 {
                        let mut message = StatusMessage::default();
                        message.severity = producer;
                        message.text[..4].copy_from_slice(&sequence.to_le_bytes());
                        assert!(queue.push(message));
                    }
                })
            })
            .collect();
        for producer in producers {
            producer.join().unwrap();
        }
        let messages: Vec<_> = (0..128).map(|_| queue.pop().unwrap()).collect();
        assert!(queue.pop().is_none());
        for producer in 0..4 {
            let mut found: Vec<_> = messages
                .iter()
                .filter(|message| message.severity == producer)
                .map(|message| u32::from_le_bytes(message.text[..4].try_into().unwrap()))
                .collect();
            found.sort_unstable();
            assert_eq!(found, (0..32).collect::<Vec<_>>());
        }
    }

    #[test]
    fn cpp_status_api_roundtrips_through_rust_owned_queue() {
        // SAFETY: The C++ smoke helper owns the process singleton and performs
        // only bounded queue push/pop operations through the public adapter.
        assert!(unsafe { hirari_status_queue_cpp_wrapper_smoke() });
    }
}
