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
pub unsafe extern "C" fn hirari_mpe_manager_clone(state: *const c_void) -> *mut c_void {
    let Some(state) = state.cast::<MpeManager>().as_ref() else { return std::ptr::null_mut() };
    Box::into_raw(Box::new(state.clone())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_manager_destroy(state: *mut c_void) {
    if !state.is_null() { drop(Box::from_raw(state.cast::<MpeManager>())); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_manager_allocate(state: *mut c_void, note: u8) -> u8 {
    let Some(state) = state.cast::<MpeManager>().as_mut() else { return 0 };
    if note > 127 { return 0; }
    for channel in 1..state.voices.len() {
        if !state.voices[channel].active {
            state.voices[channel] = Voice { note, active: true, pressure: 0.0, timbre: 0.0, pitch_bend: 0.0 };
            state.note_to_channels.entry(note).or_default().push(channel as u8);
            return channel as u8;
        }
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_manager_release(state: *mut c_void, note: u8) {
    let Some(state) = state.cast::<MpeManager>().as_mut() else { return };
    let (channel, empty) = {
        let Some(channels) = state.note_to_channels.get_mut(&note) else { return };
        let Some(channel) = channels.pop() else { return };
        (channel, channels.is_empty())
    };
    let voice = &mut state.voices[channel as usize];
    voice.active = false;
    voice.pressure = 0.0;
    voice.timbre = 0.0;
    voice.pitch_bend = 0.0;
    if empty { state.note_to_channels.remove(&note); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_manager_reset(state: *mut c_void) {
    if let Some(state) = state.cast::<MpeManager>().as_mut() { *state = MpeManager::new(); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_manager_update(
    state: *mut c_void, channel: u8, pressure: f32, timbre: f32, bend: f32,
) {
    let Some(state) = state.cast::<MpeManager>().as_mut() else { return };
    let Some(voice) = state.voices.get_mut(channel as usize) else { return };
    if channel == 0 || !voice.active { return; }
    voice.pressure = if pressure.is_finite() { pressure.clamp(0.0, 1.0) } else { 0.0 };
    voice.timbre = if timbre.is_finite() { timbre.clamp(-1.0, 1.0) } else { 0.0 };
    voice.pitch_bend = if bend.is_finite() { bend.clamp(-1.0, 1.0) } else { 0.0 };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_manager_get(
    state: *const c_void, channel: u8, note: *mut u8, active: *mut bool,
    pressure: *mut f32, timbre: *mut f32, pitch_bend: *mut f32,
) -> bool {
    let Some(state) = state.cast::<MpeManager>().as_ref() else { return false };
    let Some(voice) = state.voices.get(channel as usize) else { return false };
    if note.is_null() || active.is_null() || pressure.is_null() || timbre.is_null() || pitch_bend.is_null() { return false; }
    *note = voice.note; *active = voice.active; *pressure = voice.pressure;
    *timbre = voice.timbre; *pitch_bend = voice.pitch_bend;
    true
}
