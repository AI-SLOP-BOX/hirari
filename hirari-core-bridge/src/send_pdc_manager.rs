//! Rust-owned lifecycle for the realtime send PDC delay states.

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use std::sync::Mutex;

const MAX_NODES: usize = 128;

struct SendPdcSlot {
    handle: usize,
}

impl Drop for SendPdcSlot {
    fn drop(&mut self) {
        if self.handle != 0 {
            unsafe { hirari_send_pdc_destroy(self.handle as *mut c_void) };
        }
    }
}

struct OwnedSlots {
    active: Vec<Box<SendPdcSlot>>,
    retired: Vec<Box<SendPdcSlot>>,
}

pub struct SendPdcManager {
    slots: [[AtomicPtr<SendPdcSlot>; MAX_NODES]; MAX_NODES],
    readers: AtomicU32,
    owned: Mutex<OwnedSlots>,
}

impl SendPdcManager {
    fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| {
                std::array::from_fn(|_| AtomicPtr::new(ptr::null_mut()))
            }),
            readers: AtomicU32::new(0),
            owned: Mutex::new(OwnedSlots {
                active: Vec::new(),
                retired: Vec::new(),
            }),
        }
    }

    fn reclaim_retired(&self, owned: &mut OwnedSlots) {
        if self.readers.load(Ordering::SeqCst) == 0 {
            owned.retired.clear();
        }
    }

    fn retire(&self, source: usize, destination: usize) {
        let pointer = self.slots[source][destination].swap(ptr::null_mut(), Ordering::SeqCst);
        if pointer.is_null() {
            return;
        }
        let Ok(mut owned) = self.owned.lock() else {
            return;
        };
        if let Some(index) = owned
            .active
            .iter()
            .position(|slot| ptr::eq(&**slot, pointer))
        {
            let retired = owned.active.swap_remove(index);
            owned.retired.push(retired);
        }
        self.reclaim_retired(&mut owned);
    }

    fn retire_all(&self) {
        for row in &self.slots {
            for slot in row {
                slot.swap(ptr::null_mut(), Ordering::SeqCst);
            }
        }
        let Ok(mut owned) = self.owned.lock() else {
            return;
        };
        let active = std::mem::take(&mut owned.active);
        owned.retired.extend(active);
        self.reclaim_retired(&mut owned);
    }
}

unsafe fn manager(handle: *mut c_void) -> Option<&'static SendPdcManager> {
    if handle.is_null() {
        None
    } else {
        Some(unsafe { &*handle.cast::<SendPdcManager>() })
    }
}

unsafe extern "C" {
    fn hirari_send_pdc_create() -> *mut c_void;
    fn hirari_send_pdc_destroy(handle: *mut c_void);
    fn hirari_send_pdc_set_delay(handle: *mut c_void, samples: u32) -> bool;
    fn hirari_send_pdc_process(
        handle: *mut c_void,
        input_left: *const f32,
        input_right: *const f32,
        output_left: *mut f32,
        output_right: *mut f32,
        frames: u32,
        additional_delay: u32,
        input_gain: f32,
    ) -> bool;
}

#[no_mangle]
pub extern "C" fn hirari_send_pdc_manager_create() -> *mut c_void {
    Box::into_raw(Box::new(SendPdcManager::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_send_pdc_manager_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        unsafe { drop(Box::from_raw(handle.cast::<SendPdcManager>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_send_pdc_manager_enter_read(handle: *mut c_void) {
    if let Some(manager) = unsafe { manager(handle) } {
        manager.readers.fetch_add(1, Ordering::SeqCst);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_send_pdc_manager_leave_read(handle: *mut c_void) {
    let Some(manager) = (unsafe { manager(handle) }) else {
        return;
    };
    // The realtime callback only decrements the reader count. Retired boxes
    // are reclaimed by the next control-thread mutation or manager teardown.
    manager.readers.fetch_sub(1, Ordering::SeqCst);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_send_pdc_manager_set_delay(
    handle: *mut c_void,
    source: u32,
    destination: u32,
    samples: u32,
) -> bool {
    let Some(manager) = (unsafe { manager(handle) }) else {
        return false;
    };
    let (source, destination) = (source as usize, destination as usize);
    if source >= MAX_NODES || destination >= MAX_NODES {
        return false;
    }
    let Ok(mut owned) = manager.owned.lock() else {
        return false;
    };
    manager.reclaim_retired(&mut owned);
    let mut pointer = manager.slots[source][destination].load(Ordering::Relaxed);
    if pointer.is_null() && samples == 0 {
        return true;
    }
    if pointer.is_null() {
        let delay = unsafe { hirari_send_pdc_create() };
        if delay.is_null() {
            return false;
        }
        let mut slot = Box::new(SendPdcSlot {
            handle: delay as usize,
        });
        pointer = &mut *slot;
        owned.active.push(slot);
        manager.slots[source][destination].store(pointer, Ordering::Release);
    }
    unsafe { hirari_send_pdc_set_delay((*pointer).handle as *mut c_void, samples) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_send_pdc_manager_retire(
    handle: *mut c_void,
    source: u32,
    destination: u32,
) {
    let Some(manager) = (unsafe { manager(handle) }) else {
        return;
    };
    if source < MAX_NODES as u32 && destination < MAX_NODES as u32 {
        manager.retire(source as usize, destination as usize);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_send_pdc_manager_retire_all(handle: *mut c_void) {
    if let Some(manager) = unsafe { manager(handle) } {
        manager.retire_all();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_send_pdc_manager_process(
    handle: *mut c_void,
    source: u32,
    destination: u32,
    input_left: *const f32,
    input_right: *const f32,
    output_left: *mut f32,
    output_right: *mut f32,
    frames: u32,
    additional_delay: u32,
    input_gain: f32,
) -> bool {
    let Some(manager) = (unsafe { manager(handle) }) else {
        return false;
    };
    if source >= MAX_NODES as u32 || destination >= MAX_NODES as u32 {
        return false;
    }
    let slot = manager.slots[source as usize][destination as usize].load(Ordering::SeqCst);
    if slot.is_null() {
        return false;
    }
    unsafe {
        hirari_send_pdc_process(
            (*slot).handle as *mut c_void,
            input_left,
            input_right,
            output_left,
            output_right,
            frames,
            additional_delay,
            input_gain,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_pdc_state_is_owned_and_retired_after_readers_exit() {
        let state = SendPdcManager::new();
        let handle = (&state as *const SendPdcManager)
            .cast_mut()
            .cast::<c_void>();
        unsafe { hirari_send_pdc_manager_set_delay(handle, 3, 7, 2) };
        let input_left = [0.25_f32; 4];
        let input_right = [-0.25_f32; 4];
        let mut output_left = [0.0_f32; 4];
        let mut output_right = [0.0_f32; 4];
        state.readers.fetch_add(1, Ordering::SeqCst);
        assert!(unsafe {
            hirari_send_pdc_manager_process(
                handle,
                3,
                7,
                input_left.as_ptr(),
                input_right.as_ptr(),
                output_left.as_mut_ptr(),
                output_right.as_mut_ptr(),
                4,
                0,
                1.0,
            )
        });
        unsafe { hirari_send_pdc_manager_retire(handle, 3, 7) };
        assert_eq!(state.owned.lock().unwrap().retired.len(), 1);
        state.readers.fetch_sub(1, Ordering::SeqCst);
        {
            let mut owned = state.owned.lock().unwrap();
            state.reclaim_retired(&mut owned);
            assert!(owned.retired.is_empty());
        }
        assert!(!unsafe {
            hirari_send_pdc_manager_process(
                handle,
                3,
                7,
                input_left.as_ptr(),
                input_right.as_ptr(),
                output_left.as_mut_ptr(),
                output_right.as_mut_ptr(),
                4,
                0,
                1.0,
            )
        });
    }
}
