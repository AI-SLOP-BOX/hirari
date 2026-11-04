//! Publication store for a track's frozen render.
//!
//! C++ owns the decoded buffers but no longer tracks which one is currently
//! published. This store keeps those owners alive and publishes the buffer's
//! Rust audio-storage handle, so the realtime track runtime reads the frozen
//! block natively instead of through a C++ view type. Replaced owners stay
//! alive until the control thread says the callback has been quiesced.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

/// C++ adapter for one frozen-buffer owner (`std::shared_ptr<AudioBuffer>*`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TrackFrozenOps {
    pub destroy_owner: unsafe extern "C" fn(*mut c_void),
}

struct RetainedOwner {
    owner: *mut c_void,
    audio: *mut c_void,
}

pub struct FrozenStore {
    ops: TrackFrozenOps,
    retained: Mutex<Vec<RetainedOwner>>,
    path: Mutex<Vec<u8>>,
    current: AtomicPtr<c_void>,
    frozen: AtomicBool,
    total_samples: AtomicU64,
    sample_rate: AtomicU32,
    project_generation: AtomicU64,
    audio_generation: AtomicU64,
}

// The owner and audio pointers are only dereferenced by C++ through the ABI;
// access to the retained list and the path is serialised by the mutexes.
unsafe impl Send for FrozenStore {}
unsafe impl Sync for FrozenStore {}

impl FrozenStore {
    fn reset_publication(&self) {
        self.frozen.store(false, Ordering::Release);
        self.current.store(std::ptr::null_mut(), Ordering::Release);
        self.total_samples.store(0, Ordering::Relaxed);
        self.sample_rate.store(0, Ordering::Relaxed);
        self.project_generation.store(0, Ordering::Relaxed);
        self.audio_generation.store(0, Ordering::Relaxed);
        self.path.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    fn retire(&self) {
        let current = self.current.load(Ordering::Acquire);
        let mut retained = self.retained.lock().unwrap_or_else(|e| e.into_inner());
        let mut kept = Vec::with_capacity(retained.len());
        for entry in retained.drain(..) {
            if entry.audio == current {
                kept.push(entry);
            } else {
                unsafe { (self.ops.destroy_owner)(entry.owner) };
            }
        }
        *retained = kept;
    }
}

unsafe fn store<'a>(state: *const c_void) -> Option<&'a FrozenStore> {
    state.cast::<FrozenStore>().as_ref()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_frozen_store_create(
    ops: *const TrackFrozenOps,
) -> *mut c_void {
    let Some(ops) = ops.as_ref() else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(FrozenStore {
        ops: *ops,
        retained: Mutex::new(Vec::new()),
        path: Mutex::new(Vec::new()),
        current: AtomicPtr::new(std::ptr::null_mut()),
        frozen: AtomicBool::new(false),
        total_samples: AtomicU64::new(0),
        sample_rate: AtomicU32::new(0),
        project_generation: AtomicU64::new(0),
        audio_generation: AtomicU64::new(0),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_frozen_store_destroy(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    let store = Box::from_raw(state.cast::<FrozenStore>());
    let retained = store.retained.lock().unwrap_or_else(|e| e.into_inner());
    for entry in retained.iter() {
        (store.ops.destroy_owner)(entry.owner);
    }
}

/// Takes ownership of `owner` on success. A rejected publication leaves the
/// owner with the caller so it can release it itself.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_frozen_store_publish(
    state: *const c_void,
    owner: *mut c_void,
    audio: *mut c_void,
    channels: u32,
    frames: u64,
    total_samples: u64,
    sample_rate: u32,
    project_generation: u64,
    audio_generation: u64,
    path: *const u8,
    path_len: usize,
) -> bool {
    let Some(store) = store(state) else {
        return false;
    };
    if owner.is_null()
        || audio.is_null()
        || channels == 0
        || frames == 0
        || total_samples == 0
        || total_samples > frames
        || sample_rate == 0
    {
        return false;
    }
    let slice = if path_len == 0 {
        &[][..]
    } else if path.is_null() {
        return false;
    } else {
        std::slice::from_raw_parts(path, path_len)
    };
    store.retained.lock().unwrap_or_else(|e| e.into_inner()).push(RetainedOwner { owner, audio });
    *store.path.lock().unwrap_or_else(|e| e.into_inner()) = slice.to_vec();
    store.total_samples.store(total_samples, Ordering::Relaxed);
    store.sample_rate.store(sample_rate, Ordering::Relaxed);
    store.project_generation.store(project_generation, Ordering::Relaxed);
    store.audio_generation.store(audio_generation, Ordering::Relaxed);
    store.current.store(audio, Ordering::Release);
    store.frozen.store(true, Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_frozen_store_clear(state: *const c_void) {
    if let Some(store) = store(state) {
        store.reset_publication();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_frozen_store_retire(state: *const c_void) {
    if let Some(store) = store(state) {
        store.retire();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_frozen_store_current(state: *const c_void) -> *const c_void {
    match store(state) {
        Some(store) if store.frozen.load(Ordering::Acquire) => {
            store.current.load(Ordering::Acquire)
        }
        _ => std::ptr::null(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_frozen_store_is_frozen(state: *const c_void) -> bool {
    store(state).is_some_and(|store| store.frozen.load(Ordering::Acquire))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_frozen_store_total_samples(state: *const c_void) -> u64 {
    store(state).map_or(0, |store| store.total_samples.load(Ordering::Relaxed))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_frozen_store_sample_rate(state: *const c_void) -> u32 {
    store(state).map_or(0, |store| store.sample_rate.load(Ordering::Relaxed))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_frozen_store_project_generation(
    state: *const c_void,
) -> u64 {
    store(state).map_or(0, |store| {
        store.project_generation.load(Ordering::Relaxed)
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_frozen_store_audio_generation(state: *const c_void) -> u64 {
    store(state).map_or(0, |store| store.audio_generation.load(Ordering::Relaxed))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_frozen_store_path_len(state: *const c_void) -> usize {
    store(state).map_or(0, |store| {
        store.path.lock().unwrap_or_else(|e| e.into_inner()).len()
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_frozen_store_path_copy(
    state: *const c_void,
    out: *mut u8,
    capacity: usize,
) -> usize {
    let Some(store) = store(state) else {
        return 0;
    };
    let path = store.path.lock().unwrap_or_else(|e| e.into_inner());
    if !out.is_null() && capacity >= path.len() && !path.is_empty() {
        std::ptr::copy_nonoverlapping(path.as_ptr(), out, path.len());
    }
    path.len()
}

/// Records a cache path for an already published freeze. Rejected when the
/// store has no current publication.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_frozen_store_set_path(
    state: *const c_void,
    path: *const u8,
    path_len: usize,
) -> bool {
    let Some(store) = store(state) else {
        return false;
    };
    if !store.frozen.load(Ordering::Acquire) || path_len == 0 {
        return false;
    }
    if path.is_null() {
        return false;
    }
    *store.path.lock().unwrap_or_else(|e| e.into_inner()) =
        std::slice::from_raw_parts(path, path_len).to_vec();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    static DESTROYED: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn destroy(_owner: *mut c_void) {
        DESTROYED.fetch_add(1, AtomicOrdering::SeqCst);
    }

    fn store() -> *mut c_void {
        let ops = TrackFrozenOps {
            destroy_owner: destroy,
        };
        unsafe { hirari_track_frozen_store_create(&ops) }
    }

    #[test]
    fn publication_reports_state_and_replaces_current() {
        DESTROYED.store(0, AtomicOrdering::SeqCst);
        let handle = store();
        unsafe {
            assert!(!hirari_track_frozen_store_is_frozen(handle));
            assert!(hirari_track_frozen_store_current(handle).is_null());
            let owner = Box::into_raw(Box::new(1u8)).cast::<c_void>();
            let audio = Box::into_raw(Box::new(2u8)).cast::<c_void>();
            assert!(hirari_track_frozen_store_publish(
                handle, owner, audio, 2, 960, 960, 48000, 3, 4, b"/tmp/a.wav".as_ptr(), 10,
            ));
            assert!(hirari_track_frozen_store_is_frozen(handle));
            assert_eq!(hirari_track_frozen_store_current(handle), audio);
            assert_eq!(hirari_track_frozen_store_total_samples(handle), 960);
            assert_eq!(hirari_track_frozen_store_sample_rate(handle), 48000);
            assert_eq!(hirari_track_frozen_store_project_generation(handle), 3);
            assert_eq!(hirari_track_frozen_store_audio_generation(handle), 4);
            assert_eq!(hirari_track_frozen_store_path_len(handle), 10);
            let mut path = [0u8; 10];
            assert_eq!(hirari_track_frozen_store_path_copy(handle, path.as_mut_ptr(), path.len()), 10);
            assert_eq!(&path, b"/tmp/a.wav");

            let owner2 = Box::into_raw(Box::new(3u8)).cast::<c_void>();
            let audio2 = Box::into_raw(Box::new(4u8)).cast::<c_void>();
            assert!(hirari_track_frozen_store_publish(
                handle, owner2, audio2, 2, 480, 480, 44100, 5, 6, std::ptr::null(), 0,
            ));
            assert_eq!(hirari_track_frozen_store_current(handle), audio2);
            assert_eq!(hirari_track_frozen_store_path_len(handle), 0);
            hirari_track_frozen_store_retire(handle);
            assert_eq!(DESTROYED.load(AtomicOrdering::SeqCst), 1);

            assert!(hirari_track_frozen_store_set_path(handle, b"/cache.wav".as_ptr(), 10));
            assert_eq!(hirari_track_frozen_store_path_len(handle), 10);
            hirari_track_frozen_store_clear(handle);
            assert!(!hirari_track_frozen_store_is_frozen(handle));
            assert!(hirari_track_frozen_store_current(handle).is_null());
            assert_eq!(hirari_track_frozen_store_total_samples(handle), 0);
            assert!(!hirari_track_frozen_store_set_path(handle, b"x".as_ptr(), 1));
            hirari_track_frozen_store_retire(handle);
            assert_eq!(DESTROYED.load(AtomicOrdering::SeqCst), 2);
            hirari_track_frozen_store_destroy(handle);
        }
    }

    #[test]
    fn rejects_missing_store_and_empty_publication() {
        unsafe {
            assert!(!hirari_track_frozen_store_publish(
                std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), 2, 1, 1, 48000,
                0, 0, std::ptr::null(), 0,
            ));
            let handle = store();
            assert!(!hirari_track_frozen_store_publish(
                handle, std::ptr::null_mut(), std::ptr::null_mut(), 2, 1, 1, 48000, 0, 0,
                std::ptr::null(), 0,
            ));
            assert!(!hirari_track_frozen_store_publish(
                handle, std::ptr::null_mut(), std::ptr::null_mut(), 0, 1, 1, 48000, 0, 0,
                std::ptr::null(), 0,
            ));
            assert!(!hirari_track_frozen_store_publish(
                handle, std::ptr::null_mut(), std::ptr::null_mut(), 2, 100, 101, 48000, 0, 0,
                std::ptr::null(), 0,
            ));
            assert!(!hirari_track_frozen_store_is_frozen(handle));
            hirari_track_frozen_store_destroy(handle);
        }
    }
}
