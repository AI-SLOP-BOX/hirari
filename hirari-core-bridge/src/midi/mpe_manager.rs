use std::collections::HashMap;
use std::ffi::c_void;

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Voice { note: u8, active: bool, pressure: f32, timbre: f32, pitch_bend: f32 }

#[derive(Clone)]
struct MpeManager { voices: [Voice; 16], note_to_channels: HashMap<u8, Vec<u8>> }

impl MpeManager {
    fn new() -> Self { Self { voices: [Voice::default(); 16], note_to_channels: HashMap::new() } }
}

#[no_mangle]
pub extern "C" fn hirari_mpe_manager_create() -> *mut c_void {
    Box::into_raw(Box::new(MpeManager::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_manager_destroy(state: *mut c_void) {
    if !state.is_null() { drop(Box::from_raw(state.cast::<MpeManager>())); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_manager_reset(state: *mut c_void) {
    if let Some(state) = state.cast::<MpeManager>().as_mut() { *state = MpeManager::new(); }
}

