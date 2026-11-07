//! The track name, owned by Rust.
//!
//! The name was the last per-track field the engine had to hand to the layout
//! gather and the control edit host through a callback, because it lived as a
//! `std::string` member. Keeping it in this store means every reader reaches it
//! the same way it reaches the fader, the chain and the freeze: through the
//! track's own storage handle, with no engine code in between.
//!
//! A store is created with the track and destroyed with it, so the handle is as
//! stable as the track itself. Writes happen on the control thread; the layout
//! gather reads under the engine's tracks lock.

use std::ffi::c_void;
use std::sync::Mutex;

/// The longest name a track can carry. The limit is Rust's rule now; nothing in
/// the engine re-checks it.
pub const MAX_TRACK_NAME_BYTES: usize = 256;

#[derive(Default)]
struct NameStore {
    name: Vec<u8>,
}

fn store(state: *const c_void) -> Option<&'static Mutex<NameStore>> {
    if state.is_null() {
        return None;
    }
    Some(unsafe { &*(state as *const Mutex<NameStore>) })
}

fn lock(state: &Mutex<NameStore>) -> std::sync::MutexGuard<'_, NameStore> {
    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A name a track can actually carry: bounded and free of NUL bytes.
fn valid_name(name: &[u8]) -> bool {
    name.len() <= MAX_TRACK_NAME_BYTES && !name.contains(&0)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_name_store_create() -> *mut c_void {
    Box::into_raw(Box::new(Mutex::new(NameStore::default()))).cast::<c_void>()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_name_store_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<Mutex<NameStore>>()) });
    }
}

/// Replaces the stored name. An over-long or NUL-bearing name is refused and the
/// previous value stays, so a rejected edit can never half-apply.
///
/// # Safety
/// `state` must be a handle from [`hirari_track_name_store_create`] and `name`
/// must be readable for `name_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_name_store_set(
    state: *mut c_void,
    name: *const u8,
    name_len: usize,
) -> bool {
    let Some(state) = store(state) else {
        return false;
    };
    if name_len != 0 && name.is_null() {
        return false;
    }
    let bytes = if name_len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(name, name_len) }
    };
    if !valid_name(bytes) {
        return false;
    }
    lock(state).name = bytes.to_vec();
    true
}

/// Reports the name length; `out` receives the whole name only when it has room.
///
/// # Safety
/// `out`, if non-null, must be writable for `capacity` bytes.
unsafe fn get(state: *const c_void, out: *mut u8, capacity: usize) -> usize {
    let Some(state) = store(state) else {
        return 0;
    };
    let name = lock(state).name.clone();
    if !out.is_null() && capacity >= name.len() && !name.is_empty() {
        unsafe { std::ptr::copy_nonoverlapping(name.as_ptr(), out, name.len()) };
    }
    name.len()
}

/// Reads the whole name through the two-pass get, for Rust callers that hold a
/// store handle but no buffer. Only Rust reads names this way; C++ never does.
pub fn read(state: *const c_void) -> Vec<u8> {
    let length = unsafe { get(state, std::ptr::null_mut(), 0) };
    if length == 0 {
        return Vec::new();
    }
    let mut name = vec![0_u8; length];
    let written = unsafe { get(state, name.as_mut_ptr(), length) };
    name.truncate(written.min(length));
    name
}

/// Copies one track's name into another store with a suffix appended, which is
/// how duplicating a track names the copy. A result that would exceed the limit
/// is refused, leaving the destination untouched.
pub fn copy_with_suffix(
    source: *const c_void,
    destination: *mut c_void,
    suffix: &[u8],
) -> bool {
    let (Some(source), Some(destination)) = (store(source), store(destination)) else {
        return false;
    };
    let name = lock(source).name.clone();
    if name.len() + suffix.len() > MAX_TRACK_NAME_BYTES {
        return false;
    }
    let mut combined = name;
    combined.extend_from_slice(suffix);
    if !valid_name(&combined) {
        return false;
    }
    lock(destination).name = combined;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Store(*mut c_void);

    impl Store {
        fn new() -> Self {
            Self(unsafe { hirari_track_name_store_create() })
        }
        fn set(&self, name: &[u8]) -> bool {
            unsafe { hirari_track_name_store_set(self.0, name.as_ptr(), name.len()) }
        }
        fn read(&self) -> Vec<u8> {
            super::read(self.0 as *const c_void)
        }
    }

    impl Drop for Store {
        fn drop(&mut self) {
            unsafe { hirari_track_name_store_destroy(self.0) };
        }
    }

    #[test]
    fn a_fresh_store_holds_no_name() {
        let store = Store::new();
        assert_eq!(store.read(), Vec::<u8>::new());
    }

    #[test]
    fn names_round_trip_and_replace_one_another() {
        let store = Store::new();
        assert!(store.set(b"Drums"));
        assert_eq!(store.read(), b"Drums".to_vec());
        assert!(store.set(b"Lead Vocal"));
        assert_eq!(store.read(), b"Lead Vocal".to_vec());
    }

    #[test]
    fn an_unusable_name_is_refused_without_losing_the_current_one() {
        let store = Store::new();
        assert!(store.set(b"Bass"));
        assert!(!store.set(&vec![b'a'; MAX_TRACK_NAME_BYTES + 1]));
        assert!(!store.set(b"bad\0name"));
        assert_eq!(store.read(), b"Bass".to_vec());
    }

    #[test]
    fn a_buffer_too_small_for_the_name_is_reported_but_not_written() {
        let store = Store::new();
        assert!(store.set(b"Synth Pad"));
        let mut buffer = [0xAA_u8; 4];
        let length =
            unsafe { get(store.0 as *const c_void, buffer.as_mut_ptr(), buffer.len()) };
        assert_eq!(length, b"Synth Pad".len());
        assert_eq!(buffer, [0xAA_u8; 4]);
    }

    #[test]
    fn duplicating_appends_the_suffix_and_respects_the_limit() {
        let source = Store::new();
        let copy = Store::new();
        assert!(source.set(b"Guitar"));
        assert!(copy_with_suffix(source.0 as *const c_void, copy.0, b" Copy"));
        assert_eq!(copy.read(), b"Guitar Copy".to_vec());
        assert_eq!(source.read(), b"Guitar".to_vec());

        let long = Store::new();
        assert!(long.set(&vec![b'a'; MAX_TRACK_NAME_BYTES]));
        let rejected = Store::new();
        assert!(rejected.set(b"keep"));
        assert!(!copy_with_suffix(long.0 as *const c_void, rejected.0, b" Copy"));
        assert_eq!(rejected.read(), b"keep".to_vec());
    }

    #[test]
    fn null_handles_are_the_same_rejection_as_an_empty_store() {
        assert!(!unsafe { hirari_track_name_store_set(std::ptr::null_mut(), b"x".as_ptr(), 1) });
        assert_eq!(unsafe { get(std::ptr::null(), std::ptr::null_mut(), 0) }, 0);
        assert!(!copy_with_suffix(std::ptr::null(), std::ptr::null_mut(), b" Copy"));
        // A length with no buffer is refused rather than dereferenced.
        let store = Store::new();
        assert!(!unsafe { hirari_track_name_store_set(store.0, std::ptr::null(), 4) });
    }
}
