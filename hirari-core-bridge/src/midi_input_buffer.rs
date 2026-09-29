//! Fixed-capacity SPSC hardware MIDI input queue.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

const EVENT_CAPACITY: usize = 2048;
const EVENT_MASK: usize = EVENT_CAPACITY - 1;
const MAX_RULES: usize = 64;
const MAX_EVENTS_PER_WINDOW: u32 = 20_000;
const RATE_WINDOW: u64 = 1_000_000;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawMidiEvent {
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
    pub _padding: u8,
    pub timestamp: u64,
}

struct MidiInputBuffer {
    ring: [UnsafeCell<RawMidiEvent>; EVENT_CAPACITY],
    write_index: AtomicUsize,
    read_index: AtomicUsize,
    rules: [AtomicU32; MAX_RULES],
    rule_count: AtomicUsize,
    rate_window_start: [AtomicU64; 16],
    rate_count: [AtomicU32; 16],
    dropped_events: AtomicU64,
}

// The API contract is one MIDI producer and one audio consumer. Each side
// owns one index; release/acquire publication protects the corresponding slot.
unsafe impl Sync for MidiInputBuffer {}

impl MidiInputBuffer {
    fn new() -> Self {
        Self {
            ring: std::array::from_fn(|_| UnsafeCell::new(RawMidiEvent::default())),
            write_index: AtomicUsize::new(0),
            read_index: AtomicUsize::new(0),
            rules: std::array::from_fn(|_| AtomicU32::new(0)),
            rule_count: AtomicUsize::new(0),
            rate_window_start: std::array::from_fn(|_| AtomicU64::new(0)),
            rate_count: std::array::from_fn(|_| AtomicU32::new(0)),
            dropped_events: AtomicU64::new(0),
        }
    }

    fn accepts(&self, status: u8, data1: u8, data2: u8, timestamp: u64) -> bool {
        let count = self.rule_count.load(Ordering::Acquire).min(MAX_RULES);
        for index in 0..count {
            let packed = self.rules[index].load(Ordering::Relaxed);
            let [mask, value, rule_data1, rule_data2] = packed.to_le_bytes();
            if status & mask == value
                && (rule_data1 == 0xff || rule_data1 == data1)
                && (rule_data2 == 0xff || rule_data2 == data2)
            {
                return false;
            }
        }

        let bucket = (status & 0x0f) as usize;
        let window_start = self.rate_window_start[bucket].load(Ordering::Relaxed);
        if timestamp < window_start || timestamp - window_start >= RATE_WINDOW {
            self.rate_window_start[bucket].store(timestamp, Ordering::Relaxed);
            self.rate_count[bucket].store(0, Ordering::Relaxed);
        }
        self.rate_count[bucket].fetch_add(1, Ordering::Relaxed) < MAX_EVENTS_PER_WINDOW
    }

    fn push(&self, event: RawMidiEvent) {
        if !self.accepts(event.status, event.data1, event.data2, event.timestamp) {
            self.dropped_events.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let write = self.write_index.load(Ordering::Relaxed);
        let next = (write + 1) & EVENT_MASK;
        if next == self.read_index.load(Ordering::Acquire) {
            self.dropped_events.fetch_add(1, Ordering::Relaxed);
            return;
        }
        unsafe { *self.ring[write].get() = event };
        self.write_index.store(next, Ordering::Release);
    }

    fn pull(&self, output: &mut [RawMidiEvent]) -> usize {
        let mut read = self.read_index.load(Ordering::Relaxed);
        let write = self.write_index.load(Ordering::Acquire);
        let mut count = 0;
        while read != write && count < output.len() {
            output[count] = unsafe { *self.ring[read].get() };
            read = (read + 1) & EVENT_MASK;
            count += 1;
        }
        if count != 0 {
            self.read_index.store(read, Ordering::Release);
        }
        count
    }
}

#[no_mangle]
pub extern "C" fn hirari_midi_input_buffer_create() -> *mut c_void {
    Box::into_raw(Box::new(MidiInputBuffer::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_input_buffer_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<MidiInputBuffer>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_input_buffer_push(
    state: *const c_void,
    status: u8,
    data1: u8,
    data2: u8,
    timestamp: u64,
) {
    if let Some(state) = unsafe { state.cast::<MidiInputBuffer>().as_ref() } {
        state.push(RawMidiEvent {
            status,
            data1,
            data2,
            _padding: 0,
            timestamp,
        });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_input_buffer_add_blacklist_rule(
    state: *const c_void,
    status_mask: u8,
    status_value: u8,
    data1: u8,
    data2: u8,
) -> bool {
    let Some(state) = (unsafe { state.cast::<MidiInputBuffer>().as_ref() }) else {
        return false;
    };
    let index = state.rule_count.load(Ordering::Relaxed);
    if index >= MAX_RULES {
        return false;
    }
    state.rules[index].store(
        u32::from_le_bytes([status_mask, status_value, data1, data2]),
        Ordering::Relaxed,
    );
    state.rule_count.store(index + 1, Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_input_buffer_clear_blacklist(state: *const c_void) {
    if let Some(state) = unsafe { state.cast::<MidiInputBuffer>().as_ref() } {
        state.rule_count.store(0, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_input_buffer_dropped(state: *const c_void) -> u64 {
    unsafe { state.cast::<MidiInputBuffer>().as_ref() }
        .map_or(0, |state| state.dropped_events.load(Ordering::Relaxed))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_input_buffer_pull(
    state: *const c_void,
    output: *mut RawMidiEvent,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { state.cast::<MidiInputBuffer>().as_ref() }) else {
        return 0;
    };
    if output.is_null() || capacity == 0 {
        return 0;
    }
    let output = unsafe { std::slice::from_raw_parts_mut(output, capacity) };
    state.pull(output)
}
