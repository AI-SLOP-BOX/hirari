//! One control-thread session over a live engine track.
//!
//! Every edit host used to carry its own copy of the same prologue: take the
//! engine's tracks lock, resolve the track id, mutate, drop the frozen render,
//! bump the layout revision, release the lock. The C++ side kept that shape as
//! one lambda per edit, so each new edit host added another set of them.
//!
//! This module owns the protocol once, and owns it entirely in Rust: the engine
//! contributes its tracks lock and the address it handed to its session bag,
//! and the bag resolves the track table, the engine block and every storage a
//! track holds. A host that never created a bag — a unit test, or an engine
//! that has already closed its graph — is refused exactly as an absent host
//! table used to be.
//!
//! A session must not nest and must not outlive the edit it guards: the engine
//! lock it holds is not recursive, so calls that take it themselves — the graph
//! resync, another session — run only after the session is dropped.

use crate::engine_session::{handle_of_host, H_ENGINE_BLOCK, H_TRACKS_MUTEX, H_TRACK_TABLE};
use std::ffi::c_void;

/// Storage kinds a track exposes, matching the numbering the Rust layout gather
/// (`layout_gather.rs`) reads them with.
pub const STORAGE_STRIP: u32 = 0;
pub const STORAGE_CHAIN: u32 = 1;
pub const STORAGE_AUTOMATION: u32 = 2;
pub const STORAGE_FROZEN: u32 = 3;
pub const STORAGE_RUNTIME: u32 = 4;
pub const STORAGE_ARTICULATION: u32 = 5;
pub const STORAGE_NAME: u32 = 6;
pub const STORAGE_WORK_BUFFER: u32 = 7;

/// `Track::Type::Bus`.
pub const TRACK_TYPE_BUS: u32 = 3;

extern "C" {
    fn hirari_tracks_mutex_lock(mutex: *mut c_void);
    fn hirari_tracks_mutex_unlock(mutex: *mut c_void);
}

/// True while this host addresses an engine whose graph is still in the bag.
pub fn is_live(host: *const c_void) -> bool {
    !handle_of_host(host, H_TRACK_TABLE).is_null()
}

/// Takes the engine's tracks lock. The mutex is the engine's own, so the graph
/// is guarded against the C++ edits that still lock it directly; a bag carrying
/// no mutex belongs to a fixture with no audio thread to exclude.
pub fn lock_tracks(host: *mut c_void) {
    let mutex = handle_of_host(host as *const c_void, H_TRACKS_MUTEX);
    if !mutex.is_null() {
        unsafe { hirari_tracks_mutex_lock(mutex) };
    }
}

pub fn unlock_tracks(host: *mut c_void) {
    let mutex = handle_of_host(host as *const c_void, H_TRACKS_MUTEX);
    if !mutex.is_null() {
        unsafe { hirari_tracks_mutex_unlock(mutex) };
    }
}

/// The engine's tracks lock, held for one graph walk. The lock is not recursive,
/// so a call that takes it itself — the graph resync — runs once this drops.
pub struct GraphLock {
    host: *mut c_void,
}

impl GraphLock {
    /// Locks the engine's graph, or refuses a host whose bag has no live graph.
    pub fn acquire(host: *mut c_void) -> Option<Self> {
        if host.is_null() || !is_live(host) {
            return None;
        }
        lock_tracks(host);
        Some(Self { host })
    }

    pub fn host(&self) -> *mut c_void {
        self.host
    }
}

impl Drop for GraphLock {
    fn drop(&mut self) {
        unlock_tracks(self.host);
    }
}

/// Resolves a track id in the live graph; null when it is not there. The caller
/// holds the tracks lock.
pub fn find(host: *mut c_void, track_id: u32) -> *mut c_void {
    let table = handle_of_host(host as *const c_void, H_TRACK_TABLE);
    unsafe { crate::engine_track_table::hirari_engine_track_find(table, track_id) }
}

/// One of a track's own storages, by the numbering above.
pub fn storage(track: *mut c_void, kind: u32) -> *mut c_void {
    unsafe { crate::engine_track::hirari_track_storage(track, kind) }
}

pub fn track_type(track: *mut c_void) -> u32 {
    unsafe { crate::engine_track::hirari_track_type(track) }
}

pub fn mark_layout(track: *mut c_void) {
    unsafe { crate::engine_track::hirari_track_mark_layout(track) };
}

/// Re-prepares one track for the device format the engine block currently
/// holds.
pub fn prepare_track(host: *mut c_void, track: *mut c_void) {
    let block = handle_of_host(host as *const c_void, H_ENGINE_BLOCK);
    unsafe { crate::engine_block_host::hirari_engine_block_prepare_track(block, track) };
}

/// A live track with the engine lock held for it.
pub struct TrackSession {
    host: *mut c_void,
    track: *mut c_void,
}

impl TrackSession {
    /// Locks the engine's tracks and resolves `track_id`. Returns null-host and
    /// missing-track as the same rejection, so a caller never mutates through a
    /// stale handle.
    pub fn open(host: *mut c_void, track_id: u32) -> Option<Self> {
        let guard = GraphLock::acquire(host)?;
        let track = find(guard.host(), track_id);
        if track.is_null() {
            return None;
        }
        let host = guard.host();
        std::mem::forget(guard);
        Some(Self { host, track })
    }

    /// The track's Rust channel strip: fader, pan, solo and record-arm state.
    pub fn strip(&self) -> *mut c_void {
        storage(self.track, STORAGE_STRIP)
    }

    /// The track's Rust effect chain.
    pub fn chain(&self) -> *mut c_void {
        storage(self.track, STORAGE_CHAIN)
    }

    /// The track's Rust automation store: the volume, pan and delay curves and
    /// the plugin parameter lanes.
    pub fn automation(&self) -> *mut c_void {
        storage(self.track, STORAGE_AUTOMATION)
    }

    /// The track's Rust frozen-render store.
    pub fn frozen(&self) -> *mut c_void {
        storage(self.track, STORAGE_FROZEN)
    }

    /// The track's Rust name store.
    pub fn name_store(&self) -> *mut c_void {
        storage(self.track, STORAGE_NAME)
    }

    /// The track's Rust realtime runtime — the entry an offline freeze render
    /// drives block by block.
    pub fn runtime(&self) -> *mut c_void {
        storage(self.track, STORAGE_RUNTIME)
    }

    /// The track's active work buffer — the Rust audio storage the block
    /// pipeline renders into, which a sidechain source taps.
    pub fn work_buffer(&self) -> *mut c_void {
        storage(self.track, STORAGE_WORK_BUFFER)
    }

    /// The engine's address of this track. The Rust region store keys on it, so
    /// a caller that has pinned the track can read its region table with it.
    pub fn track(&self) -> *mut c_void {
        self.track
    }

    /// The track's current name, read from its own store.
    pub fn read_name(&self) -> Vec<u8> {
        crate::track_name_store::read(self.name_store() as *const c_void)
    }

    /// Replaces the track's name in its own store.
    pub fn write_name(&self, name: &[u8]) -> bool {
        unsafe {
            crate::track_name_store::hirari_track_name_store_set(
                self.name_store(),
                name.as_ptr(),
                name.len(),
            )
        }
    }

    pub fn is_bus(&self) -> bool {
        track_type(self.track) == TRACK_TYPE_BUS
    }

    pub fn mark_layout(&self) {
        mark_layout(self.track);
    }

    /// Re-prepares the track for the engine's current rate and block size, as
    /// the engine's initial plugin insert did.
    pub fn prepare(&self) {
        prepare_track(self.host, self.track);
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
        unlock_tracks(self.host);
    }
}
