//! One control-thread session over a live engine track.
//!
//! Every edit host used to carry its own copy of the same prologue: take the
//! engine's tracks lock, resolve the track id, mutate, drop the frozen render,
//! bump the layout revision, release the lock. The C++ side kept that shape as
//! one lambda per edit, so each new edit host added another set of them.
//!
//! This module owns the protocol once. C++ contributes only the lock, the track
//! lookup, the storage handles a track physically owns and the two steps that
//! really are engine code (re-preparing a chain for playback and marking the
//! project layout dirty). Everything between `open` and `Drop` is Rust reading
//! and writing the Rust stores the track already holds.
//!
//! A session must not nest and must not outlive the edit it guards: the engine
//! lock it holds is not recursive, so calls that take it themselves — the graph
//! resync, another session — run only after the session is dropped.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

/// Storage kinds a track exposes, matching the numbering the Rust layout gather
/// (`layout_gather.rs`) reads them with.
pub const STORAGE_STRIP: u32 = 0;
pub const STORAGE_CHAIN: u32 = 1;
pub const STORAGE_AUTOMATION: u32 = 2;
pub const STORAGE_FROZEN: u32 = 3;
pub const STORAGE_RUNTIME: u32 = 4;
pub const STORAGE_ARTICULATION: u32 = 5;

/// `Track::Type::Bus`.
pub const TRACK_TYPE_BUS: u32 = 3;

type LockFn = unsafe extern "C" fn(*mut c_void);
type TrackFn = unsafe extern "C" fn(*mut c_void, u32) -> *mut c_void;
type StorageFn = unsafe extern "C" fn(*mut c_void, u32) -> *mut c_void;
type TrackTypeFn = unsafe extern "C" fn(*mut c_void) -> u32;
type MarkFn = unsafe extern "C" fn(*mut c_void);
type PrepareFn = unsafe extern "C" fn(*mut c_void, *mut c_void);

/// The engine contributions a session needs. The table is installed once by the
/// native engine; passing null clears it.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariTrackSessionOps {
    pub lock: Option<LockFn>,
    pub unlock: Option<LockFn>,
    pub track: Option<TrackFn>,
    pub storage: Option<StorageFn>,
    pub track_type: Option<TrackTypeFn>,
    pub mark_layout: Option<MarkFn>,
    pub prepare: Option<PrepareFn>,
}

// Only function pointers supplied by the native engine, plus its host address.
unsafe impl Send for HirariTrackSessionOps {}
unsafe impl Sync for HirariTrackSessionOps {}

static HOST_OPS: OnceLock<Mutex<Option<HirariTrackSessionOps>>> = OnceLock::new();

fn host_ops() -> Option<HirariTrackSessionOps> {
    HOST_OPS.get()?.lock().ok().and_then(|guard| *guard)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_session_install(ops: *const HirariTrackSessionOps) {
    let slot = HOST_OPS.get_or_init(|| Mutex::new(None));
    let value = if ops.is_null() {
        None
    } else {
        Some(unsafe { std::ptr::read(ops) })
    };
    if let Ok(mut guard) = slot.lock() {
        *guard = value;
    }
}

/// A live track with the engine lock held for it.
pub struct TrackSession {
    ops: HirariTrackSessionOps,
    host: *mut c_void,
    track: *mut c_void,
}

impl TrackSession {
    /// Locks the engine's tracks and resolves `track_id`. Returns null-host and
    /// missing-track as the same rejection, so a caller never mutates through a
    /// stale handle.
    pub fn open(host: *mut c_void, track_id: u32) -> Option<Self> {
        let ops = host_ops()?;
        let (Some(lock), Some(unlock), Some(find)) = (ops.lock, ops.unlock, ops.track) else {
            return None;
        };
        if host.is_null() {
            return None;
        }
        unsafe { lock(host) };
        let track = unsafe { find(host, track_id) };
        if track.is_null() {
            unsafe { unlock(host) };
            return None;
        }
        Some(Self { ops, host, track })
    }

    fn storage(&self, kind: u32) -> *mut c_void {
        self.ops
            .storage
            .map_or(std::ptr::null_mut(), |resolve| unsafe { resolve(self.track, kind) })
    }

    /// The track's Rust effect chain.
    pub fn chain(&self) -> *mut c_void {
        self.storage(STORAGE_CHAIN)
    }

    /// The track's Rust frozen-render store.
    pub fn frozen(&self) -> *mut c_void {
        self.storage(STORAGE_FROZEN)
    }

    pub fn is_bus(&self) -> bool {
        self.ops
            .track_type
            .is_some_and(|kind| unsafe { kind(self.track) } == TRACK_TYPE_BUS)
    }

    pub fn mark_layout(&self) {
        if let Some(mark) = self.ops.mark_layout {
            unsafe { mark(self.track) };
        }
    }

    /// Re-prepares the track for the engine's current rate and block size, as
    /// the engine's initial plugin insert did.
    pub fn prepare(&self) {
        if let Some(prepare) = self.ops.prepare {
            unsafe { prepare(self.host, self.track) };
        }
    }

    /// Drops the rendered freeze so stale audio cannot outlive the edit, then
    /// bumps the layout revision — the protocol every control-side edit kept.
    pub fn invalidate_frozen(&self) {
        unsafe { crate::track_frozen_store::hirari_track_frozen_store_clear(self.frozen()) };
        self.mark_layout();
    }

    /// Reports `changed` after applying the edit protocol to it: a real change
    /// invalidates the frozen render and bumps the layout revision.
    pub fn commit_edit(&self, changed: bool) -> bool {
        if changed {
            self.invalidate_frozen();
            self.mark_layout();
        }
        changed
    }

    /// Applies a change that only needs the layout revision, leaving a valid
    /// frozen render alone.
    pub fn commit_layout(&self, changed: bool) -> bool {
        if changed {
            self.mark_layout();
        }
        changed
    }
}

impl Drop for TrackSession {
    fn drop(&mut self) {
        if let Some(unlock) = self.ops.unlock {
            unsafe { unlock(self.host) };
        }
    }
}
