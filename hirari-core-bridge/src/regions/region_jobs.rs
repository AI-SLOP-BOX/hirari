//! Asynchronous region jobs: media import and audio-pitch analysis.
//!
//! Decoding and F0 analysis are slow, so both run on a worker thread while the
//! control thread keeps polling. This module owns the whole lifecycle the
//! native engine used to carry: request validation, the job registry and its
//! limits, worker threads, staleness checks, the commit and the undo capture.
//! The native engine only decodes media, inserts a region, snapshots a note
//! table and runs the analysis kernel it already owns.
//!
//! Every public entry point runs on the control thread. Workers only touch
//! retained, immutable handles (decoded audio and a retained track).

use std::collections::HashMap;
use std::ffi::{c_void, CString};
use std::ptr;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

use crate::region_edit_host::{
    host_ops, record, HirariRegionEditPayload, HirariRegionHostOps, HirariRegionState,
    HIRARI_REGION_EDIT_NOTE_STORE, HIRARI_REGION_EDIT_REMOVE, HIRARI_REGION_EDIT_RESTORE,
};
use crate::undo_transaction::hirari_undo_manager_transaction_active;
use crate::vari_audio::{hirari_region_vari_audio_store_clear, HirariRegionVariAudioStore};

const STATUS_DECODING: u8 = 1;
const STATUS_READY: u8 = 2;
const STATUS_APPLIED: u8 = 3;
const STATUS_FAILED: u8 = 4;

const MAX_ACTIVE_IMPORTS: usize = 2;
const MAX_TRACKED_IMPORTS: usize = 8;
const MAX_ACTIVE_ANALYSES: usize = 2;
const MAX_PATH_BYTES: usize = 4096;
const MIN_SAMPLE_RATE: f64 = 8000.0;
const MAX_SAMPLE_RATE: f64 = 384_000.0;

/// Import result codes shared with the native wrapper, which turns them into
/// the user-visible diagnostic posts.
pub const IMPORT_OK: i32 = 0;
pub const IMPORT_TRACK_MISSING: i32 = 1;
pub const IMPORT_DECODE_FAILED: i32 = 2;
/// The media decoded but the commit was refused (rates out of range, or the
/// track was replaced while decoding). The caller reports this silently, as
/// the native commit path always did.
pub const IMPORT_COMMIT_REFUSED: i32 = 3;

/// The host pointer is only ever handed back to the host table it came from.
struct HostPtr(*mut c_void);

// The engine outlives every job: shutdown joins the workers first.
unsafe impl Send for HostPtr {}
unsafe impl Sync for HostPtr {}

fn host_key(host: *mut c_void) -> usize {
    host as usize
}

fn transaction_active(ops: &HirariRegionHostOps, host: *mut c_void) -> bool {
    let Some(undo_state) = ops.undo_state else {
        return false;
    };
    let state = unsafe { undo_state(host) };
    !state.is_null() && unsafe { hirari_undo_manager_transaction_active(state) }
}

/// An owned decoded buffer. Dropping it releases the native reference; the
/// worker only retains it until the import commits or fails.
pub(crate) struct AudioHandle {
    pub(crate) ops: HirariRegionHostOps,
    pub(crate) handle: *mut c_void,
}

unsafe impl Send for AudioHandle {}
unsafe impl Sync for AudioHandle {}

impl AudioHandle {
    fn decode(ops: &HirariRegionHostOps, path: &[u8]) -> Option<(Self, f64)> {
        // Decoding, the import cache and the planar build all live in Rust;
        // the host only frees the handle when this guard goes out of scope.
        let (handle, sample_rate, _channels, _frames) =
            crate::audio_import_cache::import_audio_file(path)?;
        Some((
            Self {
                ops: *ops,
                handle,
            },
            sample_rate,
        ))
    }

    /// Hands the buffer to the native region. The host takes ownership.
    fn into_raw(self) -> *mut c_void {
        let handle = self.handle;
        std::mem::forget(self);
        handle
    }

    fn raw(&self) -> *mut c_void {
        self.handle
    }
}

impl Drop for AudioHandle {
    fn drop(&mut self) {
        if let Some(release) = self.ops.release_audio {
            unsafe { release(self.handle) };
        }
    }
}

/// An owned note-table snapshot used for the analysis undo pair.
struct NoteStore {
    handle: *mut HirariRegionVariAudioStore,
}

unsafe impl Send for NoteStore {}
unsafe impl Sync for NoteStore {}

impl NoteStore {
    fn into_raw(self) -> *mut u8 {
        let handle = self.handle;
        std::mem::forget(self);
        handle as *mut u8
    }
}

impl Drop for NoteStore {
    fn drop(&mut self) {
        unsafe { hirari_region_vari_audio_store_clear(self.handle) };
    }
}

/// A track kept alive for the duration of an analysis worker, so removing the
/// track from the project cannot free memory the worker is still reading.
struct RetainedTrack {
    ops: HirariRegionHostOps,
    handle: *mut c_void,
    host: *mut c_void,
    track_id: u32,
}

unsafe impl Send for RetainedTrack {}
unsafe impl Sync for RetainedTrack {}

impl RetainedTrack {
    fn retain(ops: &HirariRegionHostOps, host: *mut c_void, track_id: u32) -> Option<Self> {
        let retain = ops.retain_track?;
        let handle = unsafe { retain(host, track_id) };
        if handle.is_null() {
            return None;
        }
        Some(Self {
            ops: *ops,
            handle,
            host,
            track_id,
        })
    }

    fn identity(ops: &HirariRegionHostOps, host: *mut c_void, track_id: u32) -> usize {
        match ops.track_identity {
            Some(identity) => unsafe { identity(host, track_id) },
            None => 0,
        }
    }

    fn pitch_begin(&self) -> bool {
        match self.ops.pitch_begin {
            Some(begin) => unsafe { begin(self.handle) },
            None => false,
        }
    }

    fn pitch_analyze(&self, region_id: u32, sample_rate: f64) -> bool {
        unsafe {
            crate::region_pitch::hirari_region_pitch_analyze(
                self.host,
                self.track_id,
                region_id,
                sample_rate,
            )
        }
    }

    fn pitch_finish(&self) {
        if let Some(finish) = self.ops.pitch_finish {
            unsafe { finish(self.handle) };
        }
    }

    fn note_snapshot(&self, region_id: u32) -> Option<NoteStore> {
        let snapshot = self.ops.note_snapshot?;
        let handle = unsafe { snapshot(self.handle, region_id) };
        if handle.is_null() {
            return None;
        }
        Some(NoteStore {
            handle: handle as *mut HirariRegionVariAudioStore,
        })
    }
}

impl Drop for RetainedTrack {
    fn drop(&mut self) {
        if let Some(release) = self.ops.release_track {
            unsafe { release(self.handle) };
        }
    }
}

#[path = "region_jobs_import.rs"]
mod import_;
#[path = "region_jobs_pitch.rs"]
mod pitch_;
pub use import_::*;
pub use pitch_::*;

/// Drops a project's jobs without waiting for their workers. The workers keep
/// their own references alive and their results become unreachable.
///
/// # Safety
/// `host` must be the handle the installed region host ops expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_jobs_clear(host: *mut c_void) {
    let key = host_key(host);
    if let Ok(mut registry) = import_registry().lock() {
        registry.retain(|(registry_host, _), _| *registry_host != key);
    }
    if let Ok(mut registry) = pitch_registry().lock() {
        registry.retain(|(registry_host, _), _| *registry_host != key);
    }
}

/// Joins every worker before the engine is destroyed. No job may outlive the
/// host pointer it was created with.
#[no_mangle]
pub extern "C" fn hirari_region_jobs_shutdown() {
    let import_threads = match import_registry().lock() {
        Ok(mut registry) => registry
            .drain()
            .map(|(_, job)| job.thread.lock().ok().and_then(|mut slot| slot.take()))
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    let pitch_threads = match pitch_registry().lock() {
        Ok(mut registry) => registry
            .drain()
            .map(|(_, job)| job.thread.lock().ok().and_then(|mut slot| slot.take()))
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    for thread in import_threads.into_iter().chain(pitch_threads) {
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
#[path = "region_jobs_tests.rs"]
mod tests;

