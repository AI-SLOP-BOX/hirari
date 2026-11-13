//! Publication store for a track's frozen render.
//!
//! The snapshot is a Rust audio storage produced by the freeze render
//! (`track_freeze_host.rs`). This store keeps the rendered storages alive and
//! publishes the current one through an atomic the realtime runtime loads
//! without locking. A replaced snapshot stays alive until the control thread
//! retires it after the audio callback has been quiesced.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

pub struct FrozenStore {
    retained: Mutex<Vec<*mut c_void>>,
    path: Mutex<Vec<u8>>,
    current: AtomicPtr<c_void>,
    frozen: AtomicBool,
    total_samples: AtomicU64,
    sample_rate: AtomicU32,
    project_generation: AtomicU64,
    audio_generation: AtomicU64,
}

// The storages are only ever handed to the Rust audio-buffer ABI; access to
// the retained list and the path is serialised by the mutexes.
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
        for audio in retained.drain(..) {
            if audio == current {
                kept.push(audio);
            } else {
                unsafe { crate::audio_buffer_ops::hirari_audio_buffer_storage_destroy(audio) };
            }
        }
        *retained = kept;
    }
}

unsafe fn store<'a>(state: *const c_void) -> Option<&'a FrozenStore> {
    state.cast::<FrozenStore>().as_ref()
}

#[no_mangle]
pub extern "C" fn hirari_track_frozen_store_create() -> *mut c_void {
    Box::into_raw(Box::new(FrozenStore {
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
    for audio in retained.iter() {
        crate::audio_buffer_ops::hirari_audio_buffer_storage_destroy(*audio);
    }
}

/// Takes ownership of `audio` on success. A rejected publication leaves the
/// storage with the caller so it can release it itself.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_frozen_store_publish(
    state: *const c_void,
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
    if audio.is_null()
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
    store
        .retained
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(audio);
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

/// How many rendered storages the store is still keeping alive. Everything but
/// the current publication can be released once the callback has been quiesced,
/// so this is the count the control side watches for a leak.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_frozen_store_retained_count(state: *const c_void) -> usize {
    store(state).map_or(0, |store| {
        store.retained.lock().unwrap_or_else(|e| e.into_inner()).len()
    })
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
    use crate::audio_buffer_ops::{
        hirari_audio_buffer_storage_create, hirari_audio_buffer_storage_destroy,
        hirari_audio_buffer_storage_read_pointer, hirari_audio_buffer_storage_reserve_buffer,
        hirari_audio_buffer_storage_resize, hirari_audio_buffer_storage_samples,
        hirari_audio_buffer_storage_write_pointer,
    };

    fn storage(channels: u32, samples: u32) -> *mut c_void {
        let handle = { hirari_audio_buffer_storage_create() };
        assert_eq!(unsafe { hirari_audio_buffer_storage_reserve_buffer(handle, channels, samples) }, 0);
        assert!(unsafe { hirari_audio_buffer_storage_resize(handle, channels, samples) });
        handle
    }

    fn fill(handle: *mut c_void, channel: u32, value: f32) {
        let plane = unsafe { hirari_audio_buffer_storage_write_pointer(handle, channel) };
        let frames = unsafe { hirari_audio_buffer_storage_samples(handle) };
        for index in 0..frames {
            unsafe { *plane.add(index as usize) = value };
        }
    }

    fn sample(handle: *mut c_void, channel: u32, index: u32) -> f32 {
        unsafe { *hirari_audio_buffer_storage_read_pointer(handle, channel, 0).add(index as usize) }
    }

    fn store() -> *mut c_void {
        { hirari_track_frozen_store_create() }
    }

    #[test]
    fn publication_reports_state_and_keeps_the_current_render_alive() {
        let handle = store();
        unsafe {
            assert!(!hirari_track_frozen_store_is_frozen(handle));
            assert!(hirari_track_frozen_store_current(handle).is_null());
            assert_eq!(hirari_track_frozen_store_retained_count(handle), 0);
            let first = storage(2, 960);
            fill(first, 0, 1.5);
            assert!(hirari_track_frozen_store_publish(
                handle, first, 2, 960, 960, 48000, 3, 4, b"/tmp/a.wav".as_ptr(), 10,
            ));
            assert!(hirari_track_frozen_store_is_frozen(handle));
            assert_eq!(hirari_track_frozen_store_current(handle), first);
            assert_eq!(hirari_track_frozen_store_total_samples(handle), 960);
            assert_eq!(hirari_track_frozen_store_sample_rate(handle), 48000);
            assert_eq!(hirari_track_frozen_store_project_generation(handle), 3);
            assert_eq!(hirari_track_frozen_store_audio_generation(handle), 4);
            assert_eq!(hirari_track_frozen_store_path_len(handle), 10);
            let mut path = [0u8; 10];
            assert_eq!(hirari_track_frozen_store_path_copy(handle, path.as_mut_ptr(), path.len()), 10);
            assert_eq!(&path, b"/tmp/a.wav");

            let second = storage(2, 480);
            fill(second, 0, 2.5);
            assert!(hirari_track_frozen_store_publish(
                handle, second, 2, 480, 480, 44100, 5, 6, std::ptr::null(), 0,
            ));
            assert_eq!(hirari_track_frozen_store_current(handle), second);
            assert_eq!(hirari_track_frozen_store_path_len(handle), 0);
            assert_eq!(hirari_track_frozen_store_retained_count(handle), 2);

            // Retirement releases the replaced render but never the one an
            // in-flight callback may still be reading.
            hirari_track_frozen_store_retire(handle);
            assert_eq!(hirari_track_frozen_store_retained_count(handle), 1);
            assert_eq!(sample(second, 0, 0), 2.5);

            assert!(hirari_track_frozen_store_set_path(handle, b"/cache.wav".as_ptr(), 10));
            assert_eq!(hirari_track_frozen_store_path_len(handle), 10);
            hirari_track_frozen_store_clear(handle);
            assert!(!hirari_track_frozen_store_is_frozen(handle));
            assert!(hirari_track_frozen_store_current(handle).is_null());
            assert_eq!(hirari_track_frozen_store_total_samples(handle), 0);
            assert!(!hirari_track_frozen_store_set_path(handle, b"x".as_ptr(), 1));
            hirari_track_frozen_store_retire(handle);
            assert_eq!(hirari_track_frozen_store_retained_count(handle), 0);
            hirari_track_frozen_store_destroy(handle);
        }
    }

    #[test]
    fn destroying_a_store_releases_every_render_it_still_holds() {
        let handle = store();
        unsafe {
            for frames in [64u32, 128] {
                let audio = storage(2, frames);
                assert!(hirari_track_frozen_store_publish(
                    handle, audio, 2, frames as u64, frames as u64, 48_000, 0, 0,
                    std::ptr::null(), 0,
                ));
            }
            assert_eq!(hirari_track_frozen_store_retained_count(handle), 2);
            hirari_track_frozen_store_destroy(handle);
        }
    }

    #[test]
    fn rejects_missing_store_and_empty_publication() {
        unsafe {
            assert!(!hirari_track_frozen_store_publish(
                std::ptr::null(), std::ptr::null_mut(), 2, 1, 1, 48000, 0, 0,
                std::ptr::null(), 0,
            ));
            assert_eq!(hirari_track_frozen_store_retained_count(std::ptr::null()), 0);
            let handle = store();
            let audio = storage(2, 8);
            for (channels, frames, total, rate) in
                [(0u32, 8u64, 8u64, 48_000u32), (2, 100, 101, 48_000), (2, 8, 8, 0)]
            {
                assert!(!hirari_track_frozen_store_publish(
                    handle, audio, channels, frames, total, rate, 0, 0, std::ptr::null(), 0,
                ));
            }
            // A rejected publication leaves the render with the caller.
            assert_eq!(hirari_track_frozen_store_retained_count(handle), 0);
            assert!(!hirari_track_frozen_store_is_frozen(handle));
            assert!(!hirari_track_frozen_store_publish(
                handle, std::ptr::null_mut(), 2, 8, 8, 48_000, 0, 0, std::ptr::null(), 0,
            ));
            hirari_audio_buffer_storage_destroy(audio);
            hirari_track_frozen_store_destroy(handle);
        }
    }
}