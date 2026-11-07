//! Per-thread 64-byte-aligned arena used by realtime scratch allocations.

use std::alloc::{dealloc, Layout};
use std::cell::RefCell;
use std::ptr;

const CAPACITY: usize = 1024 * 1024;
const ALIGNMENT: usize = 64;

struct Scratchpad {
    buffer: *mut u8,
    offset: usize,
}

impl Scratchpad {
    const fn new() -> Self {
        Self { buffer: ptr::null_mut(), offset: 0 }
    }

        }

impl Drop for Scratchpad {
    fn drop(&mut self) {
        if !self.buffer.is_null() {
            if let Ok(layout) = Layout::from_size_align(CAPACITY, ALIGNMENT) {
                unsafe { dealloc(self.buffer, layout) };
            }
        }
    }
}

thread_local! {
    static THREAD_SCRATCHPAD: RefCell<Scratchpad> = const { RefCell::new(Scratchpad::new()) };
}

#[no_mangle]
pub extern "C" fn hirari_forensic_scratchpad_reset() {
    THREAD_SCRATCHPAD.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.offset = 0;
        }
    });
}
