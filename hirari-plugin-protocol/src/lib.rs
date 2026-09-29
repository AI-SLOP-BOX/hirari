#![no_std]

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU64, Ordering};

pub const STATE_PROTOCOL_VERSION: u32 = 1;
pub const MAX_STATE_BYTES: usize = 4 * 1024 * 1024;
pub const STATE_ERROR_NONE: u8 = 0;
pub const STATE_ERROR_OVERSIZE: u8 = 1;
pub const STATE_ERROR_VERSION: u8 = 2;
pub const STATE_ERROR_CHECKSUM: u8 = 3;

const RING_CAPACITY: usize = 4;
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const SLOT_BYTES: usize = 16 + MAX_MESSAGE_BYTES;
const RING_STORAGE_BYTES: usize = RING_CAPACITY * SLOT_BYTES + 128;

#[repr(C)]
struct MessageSlot {
    sample_offset: u64,
    articulation_id: u8,
    padding: [u8; 3],
    size: u32,
    data: [u8; MAX_MESSAGE_BYTES],
}

#[repr(C, align(64))]
struct CacheLineCounter {
    value: AtomicU64,
}

#[repr(C, align(64))]
struct RingState {
    slots: [UnsafeCell<MessageSlot>; RING_CAPACITY],
    head: CacheLineCounter,
    tail: CacheLineCounter,
}

// The protocol contract permits exactly one producer and one consumer.
unsafe impl Sync for RingState {}

const _: () = assert!(core::mem::size_of::<MessageSlot>() == SLOT_BYTES);
const _: () = assert!(core::mem::size_of::<CacheLineCounter>() == 64);
const _: () = assert!(core::mem::size_of::<RingState>() == RING_STORAGE_BYTES);

pub fn state_checksum(data: &[u8]) -> u64 {
    data.iter()
        .fold(14_695_981_039_346_656_037_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(1_099_511_628_211)
        })
}

pub fn validate_state(data: &[u8], version: u32, expected_checksum: u64) -> u8 {
    if data.len() > MAX_STATE_BYTES {
        STATE_ERROR_OVERSIZE
    } else if version != STATE_PROTOCOL_VERSION {
        STATE_ERROR_VERSION
    } else if state_checksum(data) != expected_checksum {
        STATE_ERROR_CHECKSUM
    } else {
        STATE_ERROR_NONE
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_state_checksum(data: *const u8, size: usize) -> u64 {
    if (size != 0 && data.is_null()) || size > MAX_STATE_BYTES {
        return 0;
    }
    let mut hash = 14_695_981_039_346_656_037_u64;
    for index in 0..size {
        // SAFETY: The caller promises `size` readable bytes; null and maximum
        // length are checked above before this bounded pointer walk.
        hash ^= u64::from(unsafe { data.add(index).read() });
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_state_validate(
    data: *const u8,
    size: usize,
    version: u32,
    expected_checksum: u64,
) -> u8 {
    if size > MAX_STATE_BYTES {
        return STATE_ERROR_OVERSIZE;
    }
    if size != 0 && data.is_null() {
        return STATE_ERROR_CHECKSUM;
    }
    let mut hash = 14_695_981_039_346_656_037_u64;
    for index in 0..size {
        // SAFETY: The caller promises `size` readable bytes; null and maximum
        // length are checked above before this bounded pointer walk.
        hash ^= u64::from(unsafe { data.add(index).read() });
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    if version != STATE_PROTOCOL_VERSION {
        STATE_ERROR_VERSION
    } else if hash != expected_checksum {
        STATE_ERROR_CHECKSUM
    } else {
        STATE_ERROR_NONE
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_extended_ring_init(
    storage: *mut c_void,
    bytes: usize,
) -> bool {
    if storage.is_null() || bytes < RING_STORAGE_BYTES || (storage as usize) % 64 != 0 {
        return false;
    }
    // SAFETY: The caller gives us the full aligned opaque storage region.
    unsafe { storage.cast::<u8>().write_bytes(0, RING_STORAGE_BYTES) };
    let ring = storage.cast::<RingState>();
    // SAFETY: Zeroed storage is exclusively owned while both atomic counters
    // are constructed for the first time.
    unsafe {
        core::ptr::addr_of_mut!((*ring).head.value).write(AtomicU64::new(0));
        core::ptr::addr_of_mut!((*ring).tail.value).write(AtomicU64::new(0));
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_extended_ring_push(
    state: *mut c_void,
    sample_offset: u64,
    articulation_id: u8,
    bytes: *const u8,
    size: usize,
) -> bool {
    if state.is_null() || bytes.is_null() || size == 0 || size > MAX_MESSAGE_BYTES {
        return false;
    }
    // SAFETY: The C++ handle points to an initialized ring; the SPSC sequence
    // counters grant this producer exclusive access to the selected slot.
    let ring = unsafe { &*state.cast::<RingState>() };
    let head = ring.head.value.load(Ordering::Relaxed);
    let tail = ring.tail.value.load(Ordering::Acquire);
    if head.wrapping_sub(tail) >= RING_CAPACITY as u64 {
        return false;
    }
    // SAFETY: The producer owns this slot until it publishes the new head.
    let slot = unsafe { &mut *ring.slots[(head as usize) % RING_CAPACITY].get() };
    // SAFETY: Caller provides `size` readable bytes and size fits the slot.
    unsafe { core::ptr::copy_nonoverlapping(bytes, slot.data.as_mut_ptr(), size) };
    slot.sample_offset = sample_offset;
    slot.articulation_id = articulation_id;
    slot.size = size as u32;
    ring.head
        .value
        .store(head.wrapping_add(1), Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_extended_ring_pop(
    state: *mut c_void,
    destination_message: *mut c_void,
) -> bool {
    if state.is_null() || destination_message.is_null() {
        return false;
    }
    // SAFETY: The C++ handle points to an initialized ring; the SPSC sequence
    // counters grant this consumer exclusive access to the selected slot.
    let ring = unsafe { &*state.cast::<RingState>() };
    let tail = ring.tail.value.load(Ordering::Relaxed);
    let head = ring.head.value.load(Ordering::Acquire);
    if tail == head {
        return false;
    }
    // SAFETY: The consumer owns this slot until it publishes the new tail.
    let slot = unsafe { &*ring.slots[(tail as usize) % RING_CAPACITY].get() };
    // SAFETY: C++ destination is the exact fixed Message layout in the ABI.
    unsafe {
        core::ptr::copy_nonoverlapping(
            (slot as *const MessageSlot).cast::<u8>(),
            destination_message.cast::<u8>(),
            SLOT_BYTES,
        );
    }
    ring.tail
        .value
        .store(tail.wrapping_add(1), Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_extended_ring_size(state: *const c_void) -> usize {
    if state.is_null() {
        return 0;
    }
    // SAFETY: Caller passes a live initialized ring handle.
    let ring = unsafe { &*state.cast::<RingState>() };
    ring.head
        .value
        .load(Ordering::Acquire)
        .wrapping_sub(ring.tail.value.load(Ordering::Acquire)) as usize
}

#[cfg(feature = "standalone-static")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    unsafe extern "C" {
        fn abort() -> !;
    }
    // SAFETY: A protocol kernel panic indicates corrupt internal state. Abort
    // the isolated worker rather than returning with a possibly damaged ring.
    unsafe { abort() }
}
