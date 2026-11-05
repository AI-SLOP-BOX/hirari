//! Session registry for open native plug-in editor windows.
//!
//! The table is reached through [`HirariCore`](crate::HirariCore), which owns
//! one registry per project session, so the (track, plug-in) -> session mapping
//! lives in exactly one place regardless of which surface opens or closes an
//! editor.

use std::collections::HashMap;
use std::sync::Mutex;

pub(crate) struct SessionRegistry {
    sessions: Mutex<HashMap<u64, u64>>,
}

fn key(track: u32, plugin: u32) -> u64 {
    (u64::from(track) << 32) | u64::from(plugin)
}

impl SessionRegistry {
    pub(crate) fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn adopt(&self, track: u32, plugin: u32, session: u64) {
        if session == 0 {
            return;
        }
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.insert(key(track, plugin), session);
        }
    }

    pub(crate) fn forget(&self, track: u32, plugin: u32) -> bool {
        self.sessions
            .lock()
            .is_ok_and(|mut sessions| sessions.remove(&key(track, plugin)).is_some())
    }

    pub(crate) fn clear(&self) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.clear();
        }
    }

    pub(crate) fn session(&self, track: u32, plugin: u32) -> u64 {
        self.sessions
            .lock()
            .ok()
            .and_then(|sessions| sessions.get(&key(track, plugin)).copied())
            .unwrap_or(0)
    }

    pub(crate) fn embedded(&self, track: u32, plugin: u32) -> bool {
        self.sessions
            .lock()
            .is_ok_and(|sessions| sessions.contains_key(&key(track, plugin)))
    }
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
