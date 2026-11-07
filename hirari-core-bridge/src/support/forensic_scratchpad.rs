//! Per-thread 64-byte-aligned arena used by realtime scratch allocations.

use std::alloc::{alloc, dealloc, Layout};
use std::cell::RefCell;
use std::ffi::c_void;
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

    fn initialize(&mut self) -> bool {
        if !self.buffer.is_null() {
            return true;
        }
        let Ok(layout) = Layout::from_size_align(CAPACITY, ALIGNMENT) else {
            return false;
        };
        self.buffer = unsafe { alloc(layout) };
        !self.buffer.is_null()
    }

    fn allocate(&mut self, size: usize) -> *mut c_void {
        if !self.initialize() {
            return ptr::null_mut();
        }
        let Some(aligned_size) = size.checked_add(ALIGNMENT - 1)
            .map(|size| size & !(ALIGNMENT - 1))
        else {
            return ptr::null_mut();
        };
        let Some(end) = self.offset.checked_add(aligned_size) else {
            return ptr::null_mut();
        };
        if end > CAPACITY {
            return ptr::null_mut();
        }
        let output = unsafe { self.buffer.add(self.offset) };
        self.offset = end;
        output.cast()
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
pub extern "C" fn hirari_forensic_scratchpad_init() -> bool {
    THREAD_SCRATCHPAD.with(|state| state.borrow_mut().initialize())
}

#[no_mangle]
pub extern "C" fn hirari_forensic_scratchpad_allocate(size: usize) -> *mut c_void {
    THREAD_SCRATCHPAD.with(|state| {
        state.try_borrow_mut().map_or(ptr::null_mut(), |mut state| state.allocate(size))
    })
}

#[no_mangle]
pub extern "C" fn hirari_forensic_scratchpad_reset() {
    THREAD_SCRATCHPAD.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.offset = 0;
        }
    });
}
