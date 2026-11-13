//! MPE manager state. MPE voice routing is not implemented yet; this is the
//! session handle the engine creates and resets on project boundaries. When
//! per-voice MPE routing lands, this is where the voice table will live.

use std::ffi::c_void;

/// Reserved session handle. Intentionally empty until per-voice MPE routing
/// is implemented; keeping the allocation stable lets the engine retain one
/// long-lived handle per slot.
#[derive(Clone, Copy, Default)]
pub struct MpeManager;

impl MpeManager {
    fn new() -> Self {
        Self
    }
}

#[no_mangle]
pub extern "C" fn hirari_mpe_manager_create() -> *mut c_void {
    Box::into_raw(Box::new(MpeManager::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_manager_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<MpeManager>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_manager_reset(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<MpeManager>()) });
    }
}
