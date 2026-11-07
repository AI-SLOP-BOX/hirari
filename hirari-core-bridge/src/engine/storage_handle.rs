//! A decoded audio block that outlives the code that filled it.
//!
//! The import cache, the region store and a region's worker each hold the same
//! buffer, so its lifetime is reference-counted here rather than through a C++
//! `shared_ptr<void>` shim. The buffer itself stays the Rust storage
//! `audio_buffer_ops` owns; this only decides when it is freed.

use crate::audio_buffer_ops::{
    hirari_audio_buffer_storage_channels, hirari_audio_buffer_storage_create,
    hirari_audio_buffer_storage_destroy, hirari_audio_buffer_storage_read_pointer,
    hirari_audio_buffer_storage_samples,
};
use std::ffi::c_void;
use std::slice;
use std::sync::{Arc, Weak};

/// One owned storage; dropping the last reference frees the samples.
struct StorageRef(*mut c_void);

// The storage is a plain sample block: any thread may read or free it, and the
// engine only ever hands one out after its writer has finished with it.
unsafe impl Send for StorageRef {}
unsafe impl Sync for StorageRef {}

impl Drop for StorageRef {
    fn drop(&mut self) {
        unsafe { hirari_audio_buffer_storage_destroy(self.0) };
    }
}

type Strong = Arc<StorageRef>;
type WeakRef = Weak<StorageRef>;

fn strong(handle: *mut c_void) -> Option<&'static Strong> {
    if handle.is_null() {
        return None;
    }
    Some(unsafe { &*handle.cast::<Strong>() })
}

fn weak(handle: *mut c_void) -> Option<&'static WeakRef> {
    if handle.is_null() {
        return None;
    }
    Some(unsafe { &*handle.cast::<WeakRef>() })
}

/// A new owned buffer. `out_storage` receives the storage to resize and fill.
#[no_mangle]
pub extern "C" fn hirari_audio_import_buffer_create(out_storage: *mut *mut c_void) -> *mut c_void {
    let storage = hirari_audio_buffer_storage_create();
    if !out_storage.is_null() {
        unsafe { *out_storage = storage };
    }
    Box::into_raw(Box::new(Arc::new(StorageRef(storage)))).cast()
}

#[no_mangle]
pub extern "C" fn hirari_audio_import_buffer_storage(audio: *mut c_void) -> *mut c_void {
    strong(audio)
        .map(|handle| handle.0)
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn hirari_audio_import_buffer_weak(audio: *mut c_void) -> *mut c_void {
    strong(audio).map(|handle| Box::into_raw(Box::new(Arc::downgrade(handle))).cast() as *mut c_void)
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_import_buffer_release(audio: *mut c_void) {
    if !audio.is_null() {
        drop(Box::from_raw(audio.cast::<Strong>()));
    }
}

/// A weak handle's buffer, kept alive by the returned strong handle.
#[no_mangle]
pub extern "C" fn hirari_audio_import_weak_lock(weak_audio: *mut c_void) -> *mut c_void {
    weak(weak_audio)
        .and_then(|handle| handle.upgrade())
        .map(|handle| Box::into_raw(Box::new(handle)).cast() as *mut c_void)
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn hirari_audio_import_weak_alive(weak_audio: *mut c_void) -> bool {
    weak(weak_audio).is_some_and(|handle| handle.strong_count() != 0)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_import_weak_release(weak_audio: *mut c_void) {
    if !weak_audio.is_null() {
        drop(Box::from_raw(weak_audio.cast::<WeakRef>()));
    }
}

/// Another owner for an already-owned buffer.
#[no_mangle]
pub extern "C" fn hirari_storage_handle_retain(audio: *mut c_void) -> *mut c_void {
    strong(audio)
        .map(|handle| Box::into_raw(Box::new(Arc::clone(handle))).cast() as *mut c_void)
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_storage_handle_release(audio: *mut c_void) {
    hirari_audio_import_buffer_release(audio);
}

/// The buffer's channel count; with room in `out`, also its plane pointers, and
/// `samples` its frame count.
#[no_mangle]
pub unsafe extern "C" fn hirari_storage_handle_view(
    audio: *mut c_void,
    out: *mut *const f32,
    capacity: usize,
    samples: *mut u64,
) -> u32 {
    let Some(storage) = strong(audio).map(|handle| handle.0).filter(|buffer| !buffer.is_null())
    else {
        return 0;
    };
    let channels = unsafe { hirari_audio_buffer_storage_channels(storage) };
    if !samples.is_null() {
        unsafe { *samples = u64::from(hirari_audio_buffer_storage_samples(storage)) };
    }
    if !out.is_null() && capacity >= channels as usize {
        let planes = unsafe { slice::from_raw_parts_mut(out, channels as usize) };
        for (channel, plane) in planes.iter_mut().enumerate() {
            *plane = unsafe { hirari_audio_buffer_storage_read_pointer(storage, channel as u32, 0) };
        }
    }
    channels
}
