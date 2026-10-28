//! Session registry for open native plug-in editor windows.
//!
//! The platform callbacks stay in C++ because they own the window toolkit, but
//! the (track, plug-in) -> session mapping and its key packing live here so the
//! engine keeps a single owner for editor bookkeeping.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;

struct SessionRegistry {
    sessions: Mutex<HashMap<u64, u64>>,
}

fn key(track: u32, plugin: u32) -> u64 {
    (u64::from(track) << 32) | u64::from(plugin)
}

impl SessionRegistry {
    fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn adopt(&self, track: u32, plugin: u32, session: u64) {
        if session == 0 {
            return;
        }
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.insert(key(track, plugin), session);
        }
    }

    fn forget(&self, track: u32, plugin: u32) -> bool {
        self.sessions
            .lock()
            .is_ok_and(|mut sessions| sessions.remove(&key(track, plugin)).is_some())
    }

    fn clear(&self) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.clear();
        }
    }

    fn session(&self, track: u32, plugin: u32) -> u64 {
        self.sessions
            .lock()
            .ok()
            .and_then(|sessions| sessions.get(&key(track, plugin)).copied())
            .unwrap_or(0)
    }

    fn embedded(&self, track: u32, plugin: u32) -> bool {
        self.sessions
            .lock()
            .is_ok_and(|sessions| sessions.contains_key(&key(track, plugin)))
    }
}

fn state<'a>(handle: *const c_void) -> Option<&'a SessionRegistry> {
    unsafe { handle.cast::<SessionRegistry>().as_ref() }
}

#[no_mangle]
pub extern "C" fn hirari_native_editor_sessions_create() -> *mut c_void {
    Box::into_raw(Box::new(SessionRegistry::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_editor_sessions_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        // SAFETY: handle comes from the matching create function and is dropped once.
        unsafe { drop(Box::from_raw(handle.cast::<SessionRegistry>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_editor_sessions_clear(handle: *mut c_void) {
    if let Some(state) = state(handle) {
        state.clear();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_editor_sessions_adopt(
    handle: *mut c_void,
    track: u32,
    plugin: u32,
    session: u64,
) {
    if let Some(state) = state(handle) {
        state.adopt(track, plugin, session);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_editor_sessions_forget(
    handle: *mut c_void,
    track: u32,
    plugin: u32,
) -> bool {
    state(handle).is_some_and(|state| state.forget(track, plugin))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_editor_sessions_embedded(
    handle: *const c_void,
    track: u32,
    plugin: u32,
) -> bool {
    state(handle).is_some_and(|state| state.embedded(track, plugin))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_editor_sessions_session(
    handle: *const c_void,
    track: u32,
    plugin: u32,
) -> u64 {
    state(handle).map_or(0, |state| state.session(track, plugin))
}

#[cfg(test)]
mod tests {
    use super::SessionRegistry;

    #[test]
    fn sessions_are_keyed_by_track_and_plugin() {
        let registry = SessionRegistry::new();
        assert!(!registry.embedded(3, 4));
        assert_eq!(registry.session(3, 4), 0);
        registry.adopt(3, 4, 11);
        registry.adopt(3, 5, 12);
        registry.adopt(4, 4, 13);
        assert_eq!(registry.session(3, 4), 11);
        assert_eq!(registry.session(3, 5), 12);
        assert_eq!(registry.session(4, 4), 13);
        assert!(registry.embedded(4, 4));
        assert!(registry.forget(3, 4));
        assert!(!registry.forget(3, 4));
        assert_eq!(registry.session(3, 4), 0);
        assert_eq!(registry.session(3, 5), 12);
    }

    #[test]
    fn zero_sessions_are_ignored_and_clear_empties_the_table() {
        let registry = SessionRegistry::new();
        registry.adopt(1, 2, 0);
        assert!(!registry.embedded(1, 2));
        registry.adopt(1, 2, 9);
        registry.clear();
        assert!(!registry.embedded(1, 2));
        assert_eq!(registry.session(1, 2), 0);
    }
}
