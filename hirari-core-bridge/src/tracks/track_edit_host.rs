//! Track lifecycle policy: create, duplicate and remove, with their undo entries.
//!
//! The graph is Rust's already: the track table (`engine_track_table.rs`), the
//! sidechain links and routes and the routing engine all answer to handles the
//! engine hands out, and the shared track session (`track_session.rs`) lends its
//! lock, its track lookup and the storages each track owns. So this module walks
//! the project itself — it names a new track, holds the graph lock while it moves
//! the table, publishes the audio-thread snapshot, captures and replays the links,
//! routes and connections, and hands one entry to the history.
//!
//! Every entry point runs on the control thread.

use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock};

use crate::bus_audio::{hirari_bus_system_bus, hirari_bus_system_register};
use crate::engine_block_host::{
    hirari_engine_block_device_block_frames, hirari_engine_block_device_sample_rate,
    hirari_engine_block_generation,
};
use crate::engine_track::{
    hirari_track_new, hirari_track_owner_object, hirari_track_owner_release,
    hirari_track_owner_retain, hirari_track_prepare, hirari_track_resolve_bus, TRACK_TYPE_BUS,
};
use crate::engine_track_table::{
    hirari_engine_track_count, hirari_engine_track_detach, hirari_engine_track_find,
    hirari_engine_track_index_of, hirari_engine_track_insert, hirari_engine_track_publish,
};
use crate::plugin_edit_host::{commit_closures, UndoStateFn};
use crate::region_edit_host::{HirariRegionListRegionsFn, HirariRegionListTracksFn};
use crate::region_store::{
    hirari_region_store_copy_regions, hirari_region_store_count,
    hirari_region_store_forget_waveforms,
};
use crate::routing_engine_core::{
    hirari_routing_connection_array_free, hirari_routing_engine_add_connection,
    hirari_routing_engine_add_feedback, hirari_routing_engine_remove_connections_for_node,
    hirari_routing_engine_remove_dependencies_for_node,
    hirari_routing_engine_remove_feedback_for_node, hirari_routing_feedback_array_free,
};
use crate::routing_feedback::FeedbackConnection;
use crate::routing_graph_pdc::RoutingConnectionRecord;
use crate::sidechain_manager::{
    hirari_sidechain_engine_capture_links, hirari_sidechain_engine_free_captured_links,
    hirari_sidechain_engine_remove_track, hirari_sidechain_engine_restore_links,
};
use crate::sidechain_route_store::{
    hirari_sidechain_routes_capture_track, hirari_sidechain_routes_erase_track,
    hirari_sidechain_routes_release, hirari_sidechain_routes_restore,
};
use crate::track_session::{
    self, STORAGE_AUTOMATION, STORAGE_CHAIN, STORAGE_NAME, STORAGE_RUNTIME, STORAGE_STRIP,
};

/// The engine stores a lifecycle walk visits, as `store` numbers them.
pub const STORE_TRACK_TABLE: u32 = 0;
pub const STORE_ENGINE_BLOCK: u32 = 1;
pub const STORE_SIDECHAIN: u32 = 2;
pub const STORE_SIDECHAIN_ROUTES: u32 = 3;
pub const STORE_ROUTING: u32 = 4;
pub const STORE_LAYOUT_REVISION: u32 = 5;
pub const STORE_BUS_SYSTEM: u32 = 6;

/// One of the engine's Rust-owned stores, by `STORE_*` kind.
pub type StoreFn = unsafe extern "C" fn(*mut c_void, u32) -> *mut c_void;
pub type ReplayTrackEditFn = unsafe extern "C" fn(*mut c_void);
/// Records an applied action; the host invokes `undo`/`redo` with `edit` and
/// releases it once the history drops the entry.
pub type RecordTrackReplayFn = unsafe extern "C" fn(
    *mut c_void,
    *const u8,
    usize,
    *mut c_void,
    ReplayTrackEditFn,
    ReplayTrackEditFn,
    ReplayTrackEditFn,
);
/// Re-publishes the audio-thread track snapshot and rebuilds the graph.
pub type SyncStructuralChangesFn = unsafe extern "C" fn(*mut c_void);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariTrackEditHostOps {
    /// Lists track ids / one track's region ids in storage order; the id
    /// allocator in `id_host` reads the project through these.
    pub list_tracks: Option<HirariRegionListTracksFn>,
    pub list_regions: Option<HirariRegionListRegionsFn>,
    pub store: Option<StoreFn>,
    pub record_replay: Option<RecordTrackReplayFn>,
    pub sync_structural_changes: Option<SyncStructuralChangesFn>,
    pub undo_state: Option<UndoStateFn>,
}

// The table only holds function pointers supplied by the native engine.
unsafe impl Send for HirariTrackEditHostOps {}
unsafe impl Sync for HirariTrackEditHostOps {}

static HOST_OPS: OnceLock<Mutex<Option<HirariTrackEditHostOps>>> = OnceLock::new();

fn host_ops() -> Option<HirariTrackEditHostOps> {
    HOST_OPS.get()?.lock().ok().and_then(|guard| *guard)
}

/// Installs the native host callbacks. Passing null clears them.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_edit_host_install(ops: *const HirariTrackEditHostOps) {
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

/// The handles one lifecycle walk visits, resolved from the engine host.
#[path = "track_edit_capture.rs"]
mod capture;
#[path = "track_edit_chain.rs"]
mod chain;
pub use capture::*;
pub use chain::*;

fn add_track_impl(
    ops: &HirariTrackEditHostOps,
    host: *mut c_void,
    track_type: u32,
) -> u32 {
    let Some(stores) = Stores::new(ops, host) else {
        return 0;
    };
    let id = crate::id_host::next_track_id(ops.list_tracks, host);
    if id == 0 {
        return 0;
    }
    let name = if track_type == TRACK_TYPE_BUS {
        format!("Bus {}", id.wrapping_add(1))
    } else {
        format!("Track {}", id.wrapping_add(1))
    };
    let captured = {
        // Building the track and appending it stay inside one lock hold, which
        // is where the engine's own create path had both.
        let _guard = stores.lock();
        let (owner, track) = stores.build(track_type, id, &name);
        let owner = TrackOwner { owner };
        let index = stores.append(owner.owner);
        stores.publish(true);
        owner.into_captured(track, index, id)
    };
    record_track_edit(
        ops,
        host,
        "Add Track",
        TrackEdit {
            ops: *ops,
            host,
            kind: TrackEditKind::Created {
                track: captured,
                // An undone add drops the new track's sidechain links, and its
                // redo stamps the audio generation the append published with.
                drop_sidechain_links: true,
                stamp_generation: true,
            },
        },
    );
    sync_structural_changes(ops, host);
    id
}

fn duplicate_track_impl(
    ops: &HirariTrackEditHostOps,
    host: *mut c_void,
    track_id: u32,
) -> u32 {
    let Some(stores) = Stores::new(ops, host) else {
        return 0;
    };
    let new_id = crate::id_host::next_track_id(ops.list_tracks, host);
    if new_id == 0 {
        return 0;
    }
    // The copy's regions each need an id of their own; the batch has to cover
    // every region the source track holds before any of them is cloned.
    let region_count = crate::id_host::region_count(ops.list_regions, host, track_id);
    let region_ids =
        crate::id_host::next_region_ids(ops.list_tracks, ops.list_regions, host, region_count);
    if region_ids.len() != region_count {
        return 0;
    }
    let captured = {
        let _guard = stores.lock();
        let source = stores.find(track_id);
        if source.is_null() || hirari_region_store_count(source) != region_ids.len() {
            return 0;
        }
        // The copy is named by the clone step, so it starts nameless.
        let (owner, copy) = stores.build(track_session::track_type(source), new_id, "");
        let owner = TrackOwner { owner };
        // The copy's own storages are the destination, name included: the reads
        // and writes go store to store, so the copy suffix is Rust's rule.
        let cloned = unsafe {
            hirari_track_clone_state(
                track_session::storage(source, STORAGE_STRIP),
                track_session::storage(source, STORAGE_RUNTIME),
                track_session::storage(source, STORAGE_AUTOMATION),
                track_session::storage(source, STORAGE_CHAIN),
                track_session::storage(source, STORAGE_NAME),
                track_session::storage(copy, STORAGE_STRIP),
                track_session::storage(copy, STORAGE_RUNTIME),
                track_session::storage(copy, STORAGE_AUTOMATION),
                track_session::storage(copy, STORAGE_CHAIN),
                track_session::storage(copy, STORAGE_NAME),
                track_session::track_type(copy) == TRACK_TYPE_BUS,
            )
        };
        let regions =
            unsafe { hirari_region_store_copy_regions(source, copy, region_ids.as_ptr(), region_ids.len()) };
        if !cloned || !regions {
            return 0;
        }
        let index = stores.append(owner.owner);
        stores.publish(false);
        owner.into_captured(copy, index, new_id)
    };
    record_track_edit(
        ops,
        host,
        "Duplicate Track",
        TrackEdit {
            ops: *ops,
            host,
            kind: TrackEditKind::Created {
                track: captured,
                // The duplicate path never dropped sidechain links and never
                // stamped the audio generation, matching the engine exactly.
                drop_sidechain_links: false,
                stamp_generation: false,
            },
        },
    );
    sync_structural_changes(ops, host);
    new_id
}

fn remove_track_impl(ops: &HirariTrackEditHostOps, host: *mut c_void, track_id: u32) -> bool {
    let Some(stores) = Stores::new(ops, host) else {
        return false;
    };
    let captured = {
        // The links are captured before the graph is touched, as the engine's
        // take did, so a link another thread dropped in between is still replayed.
        let links = unsafe { hirari_sidechain_engine_capture_links(stores.sidechain, track_id) };
        let _guard = stores.lock();
        let index = unsafe { hirari_engine_track_index_of(stores.table, track_id) };
        if index == usize::MAX {
            unsafe { hirari_sidechain_engine_free_captured_links(links) };
            return false;
        }
        unsafe { hirari_sidechain_engine_remove_track(stores.sidechain, track_id) };
        let routes = unsafe { hirari_sidechain_routes_capture_track(stores.routes, track_id) };
        unsafe { hirari_routing_engine_remove_dependencies_for_node(stores.routing, track_id) };
        let routing = take_routing_records(&stores, track_id);
        let feedback = take_feedback_records(&stores, track_id);
        let track = stores.find(track_id);
        let owner = stores.detach(track_id);
        let captured = CapturedTrack {
            owner,
            track,
            index,
            id: track_id,
            sidechain_links: links,
            sidechain_routes: routes,
            routing,
            feedback,
        };
        if owner.is_null() {
            return false;
        }
        captured
    };
    // Removing the bootstrap placeholder (ID 1) from an otherwise empty project
    // rewinds the counter so the first track a document installs keeps its
    // canonical ID instead of becoming ID 2.
    if track_id == 1 {
        crate::id_host::rewind_track_counter(ops.list_tracks, host);
    }
    record_track_edit(
        ops,
        host,
        "Remove Track",
        TrackEdit {
            ops: *ops,
            host,
            kind: TrackEditKind::Removed { track: captured },
        },
    );
    sync_structural_changes(ops, host);
    true
}

/// Creates a track, names it from its allocated id and records one
/// `Add Track` entry.
///
/// The track is named `Track <id+1>`, or `Bus <id+1>` for a bus, and appended to
/// the graph. The entry's undo removes it and its redo re-inserts it at the same
/// index. Returns the new id, or 0 when no id is available.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_add(host: *mut c_void, track_type: u32) -> u32 {
    match host_ops() {
        Some(ops) => add_track_impl(&ops, host, track_type),
        None => 0,
    }
}

/// Duplicates a track and records one `Duplicate Track` entry.
///
/// The copy inherits the source's deep state — plugins, automation, regions and
/// MIDI — and takes `<source> Copy` as its name. Returns the new id, or 0 when
/// the source track is missing or its regions cannot be cloned.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_duplicate(host: *mut c_void, track_id: u32) -> u32 {
    match host_ops() {
        Some(ops) => duplicate_track_impl(&ops, host, track_id),
        None => 0,
    }
}

/// Removes a track and records one `Remove Track` entry whose undo restores it,
/// with its index, its sidechain links, its routes and its routing connections.
///
/// Returns false, recording nothing, when the track is missing.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_remove(host: *mut c_void, track_id: u32) -> bool {
    match host_ops() {
        Some(ops) => remove_track_impl(&ops, host, track_id),
        None => false,
    }
}
/// Reads one processor state out of a chain, which the caller owns once copied.

#[cfg(test)]
#[path = "track_edit_host_tests.rs"]
mod tests;

