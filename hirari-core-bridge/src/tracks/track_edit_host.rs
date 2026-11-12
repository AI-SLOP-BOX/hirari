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
struct Stores {
    host: *mut c_void,
    block: *mut c_void,
    table: *mut c_void,
    revision: *mut c_void,
    bus_system: *mut c_void,
    sidechain: *mut c_void,
    routes: *mut c_void,
    routing: *mut c_void,
}

impl Stores {
    fn new(ops: &HirariTrackEditHostOps, host: *mut c_void) -> Option<Self> {
        if !track_session::is_live(host) {
            return None;
        }
        let store = ops.store?;
        Some(Self {
            host,
            block: unsafe { store(host, STORE_ENGINE_BLOCK) },
            table: unsafe { store(host, STORE_TRACK_TABLE) },
            revision: unsafe { store(host, STORE_LAYOUT_REVISION) },
            bus_system: unsafe { store(host, STORE_BUS_SYSTEM) },
            sidechain: unsafe { store(host, STORE_SIDECHAIN) },
            routes: unsafe { store(host, STORE_SIDECHAIN_ROUTES) },
            routing: unsafe { store(host, STORE_ROUTING) },
        })
    }

    fn lock(&self) -> Option<track_session::GraphLock> {
        track_session::GraphLock::acquire(self.host)
    }

    fn find(&self, track_id: u32) -> *mut c_void {
        unsafe { hirari_engine_track_find(self.table, track_id) }
    }

    /// Releases one owning handle this walk no longer needs.
    fn release(&self, owner: *mut c_void) {
        if !owner.is_null() {
            unsafe { hirari_track_owner_release(owner) };
        }
    }

    /// Republishes the graph at the engine block's current generation. `stamp`
    /// decides whether that generation travels with the publication: an edit
    /// that did not change the audio format keeps the previous value, so a
    /// command queued for the old generation is never applied to a new one.
    fn publish(&self, stamp: bool) {
        unsafe {
            hirari_engine_track_publish(self.table, hirari_engine_block_generation(self.block), stamp)
        };
    }

    /// Builds a track of this kind and id, names it, and brings it up at the
    /// device format the graph is prepared for. A bus takes its slot from the
    /// engine's bus system first: the structural walk can only route to a slot
    /// the system actually hands out, so an unresolvable bus stays slotless.
    fn build(&self, kind: u32, id: u32, name: &str) -> (*mut c_void, *mut c_void) {
        let mut track = std::ptr::null_mut();
        let owner = unsafe {
            hirari_track_new(
                id,
                name.as_ptr(),
                name.len(),
                kind,
                self.revision,
                &mut track,
            )
        };
        if kind == TRACK_TYPE_BUS {
            let state = unsafe {
                hirari_bus_system_register(self.bus_system, id)
                    .then(|| hirari_bus_system_bus(self.bus_system, id))
            };
            unsafe {
                hirari_track_resolve_bus(track, state.unwrap_or(std::ptr::null_mut()), true)
            };
        }
        unsafe {
            hirari_track_prepare(
                track,
                hirari_engine_block_device_sample_rate(self.block),
                hirari_engine_block_device_block_frames(self.block),
            )
        };
        (owner, track)
    }

    /// Appends a track the walk built and returns the index it landed at. The
    /// caller keeps its own handle: the undo capture outlives the graph, and the
    /// table holds a reference of its own.
    fn append(&self, owner: *mut c_void) -> usize {
        let index = unsafe { hirari_engine_track_count(self.table) };
        let track = unsafe { hirari_track_owner_object(owner) };
        let owned = unsafe { hirari_track_owner_retain(owner) };
        if unsafe { hirari_engine_track_insert(self.table, index, owned) } {
            track_session::mark_layout(track);
        }
        index
    }

    /// Takes a track out of the graph and hands back its owning handle, dropping
    /// the waveforms its regions referenced and running the edit protocol the
    /// engine's own detach kept. Null when the id is not in the graph.
    fn detach(&self, track_id: u32) -> *mut c_void {
        let track = self.find(track_id);
        if track.is_null() {
            return std::ptr::null_mut();
        }
        hirari_region_store_forget_waveforms(self.host, track);
        // The table keeps a reference of its own in the deferred list, so the
        // caller gets a second one while a device callback may still hold the
        // raw pointer.
        let owner = unsafe { hirari_engine_track_detach(self.table, track_id) };
        if owner.is_null() {
            return std::ptr::null_mut();
        }
        track_session::mark_layout(track);
        self.publish(false);
        owner
    }

    /// Puts a captured track back at its index, then runs the same protocol.
    /// Restoring an id that is already in the graph is a no-op, and the table
    /// clamps an out-of-range index to the end.
    fn restore_at(&self, captured: &CapturedTrack, stamp: bool) {
        if !self.find(captured.id).is_null() {
            return;
        }
        let owner = unsafe { hirari_track_owner_retain(captured.owner) };
        if unsafe { hirari_engine_track_insert(self.table, captured.index, owner) } {
            track_session::mark_layout(captured.track);
            self.publish(stamp);
        }
    }
}

/// An owning handle this walk built, released when the edit it was built for
/// does not complete.
struct TrackOwner {
    owner: *mut c_void,
}

impl TrackOwner {
    /// Hands the handle to a capture that takes over the release.
    fn into_captured(self, track: *mut c_void, index: usize, id: u32) -> CapturedTrack {
        let kept = std::mem::ManuallyDrop::new(self);
        CapturedTrack::created(kept.owner, track, index, id)
    }
}

impl Drop for TrackOwner {
    fn drop(&mut self) {
        if !self.owner.is_null() {
            unsafe { hirari_track_owner_release(self.owner) };
        }
    }
}

/// Removes one node's routing connections and owns them for the undo entry.
fn take_routing_records(stores: &Stores, track_id: u32) -> Vec<RoutingConnectionRecord> {
    let mut count = 0usize;
    let records =
        unsafe { hirari_routing_engine_remove_connections_for_node(stores.routing, track_id, &mut count) };
    if records.is_null() {
        return Vec::new();
    }
    let taken = unsafe { std::slice::from_raw_parts(records, count) }.to_vec();
    unsafe { hirari_routing_connection_array_free(records, count) };
    taken
}

/// Removes one node's feedback connections and owns them for the undo entry.
fn take_feedback_records(stores: &Stores, track_id: u32) -> Vec<FeedbackConnection> {
    let mut count = 0usize;
    let records =
        unsafe { hirari_routing_engine_remove_feedback_for_node(stores.routing, track_id, &mut count) };
    if records.is_null() {
        return Vec::new();
    }
    let taken = unsafe { std::slice::from_raw_parts(records, count) }.to_vec();
    unsafe { hirari_routing_feedback_array_free(records, count) };
    taken
}

/// Drops whatever a removed node still owned, with no undo record kept for it.
fn discard_routing(stores: &Stores, track_id: u32) {
    let mut count = 0usize;
    let records =
        unsafe { hirari_routing_engine_remove_connections_for_node(stores.routing, track_id, &mut count) };
    unsafe { hirari_routing_connection_array_free(records, count) };
    let mut feedback_count = 0usize;
    let records =
        unsafe { hirari_routing_engine_remove_feedback_for_node(stores.routing, track_id, &mut feedback_count) };
    unsafe { hirari_routing_feedback_array_free(records, feedback_count) };
}

/// Everything one lifecycle edit holds so undo can put the track back exactly:
/// its owning handle and address, the graph index it came from, and the captures
/// of the sidechain links, routes and routing connections taken with it.
pub struct CapturedTrack {
    owner: *mut c_void,
    track: *mut c_void,
    index: usize,
    id: u32,
    sidechain_links: *mut c_void,
    sidechain_routes: *mut c_void,
    routing: Vec<RoutingConnectionRecord>,
    feedback: Vec<FeedbackConnection>,
}

impl CapturedTrack {
    /// A track the edit just built: nothing was taken out of the graph with it,
    /// so undo only has to remove it again.
    fn created(owner: *mut c_void, track: *mut c_void, index: usize, id: u32) -> Self {
        Self {
            owner,
            track,
            index,
            id,
            sidechain_links: std::ptr::null_mut(),
            sidechain_routes: std::ptr::null_mut(),
            routing: Vec::new(),
            feedback: Vec::new(),
        }
    }
}

impl Drop for CapturedTrack {
    fn drop(&mut self) {
        unsafe {
            hirari_sidechain_engine_free_captured_links(self.sidechain_links);
            hirari_sidechain_routes_release(self.sidechain_routes);
        }
        if !self.owner.is_null() {
            unsafe { hirari_track_owner_release(self.owner) };
        }
    }
}

enum TrackEditKind {
    /// A track was created or duplicated; undo removes it, redo restores it.
    Created {
        track: CapturedTrack,
        drop_sidechain_links: bool,
        stamp_generation: bool,
    },
    /// A track was removed; undo restores it with its links, redo removes it.
    Removed { track: CapturedTrack },
}

/// One lifecycle undo entry. The host table is copied in so replay never
/// depends on the current global.
struct TrackEdit {
    ops: HirariTrackEditHostOps,
    host: *mut c_void,
    kind: TrackEditKind,
}

unsafe impl Send for TrackEdit {}

unsafe extern "C" fn replay_track_undo(edit: *mut c_void) {
    if edit.is_null() {
        return;
    }
    let edit = unsafe { &*(edit as *const TrackEdit) };
    let Some(stores) = Stores::new(&edit.ops, edit.host) else {
        return;
    };
    match &edit.kind {
        TrackEditKind::Created {
            track,
            drop_sidechain_links,
            ..
        } => remove_captured(&stores, track, *drop_sidechain_links, false),
        TrackEditKind::Removed { track } => restore_captured(&stores, track, false),
    }
}

unsafe extern "C" fn replay_track_redo(edit: *mut c_void) {
    if edit.is_null() {
        return;
    }
    let edit = unsafe { &*(edit as *const TrackEdit) };
    let Some(stores) = Stores::new(&edit.ops, edit.host) else {
        return;
    };
    match &edit.kind {
        TrackEditKind::Created {
            track,
            stamp_generation,
            ..
        } => restore_captured(&stores, track, *stamp_generation),
        TrackEditKind::Removed { track } => remove_captured(&stores, track, true, true),
    }
}

unsafe extern "C" fn release_track_edit(edit: *mut c_void) {
    if edit.is_null() {
        return;
    }
    // Dropping the entry releases the captured track's handle, its sidechain
    // links and its route capture.
    drop(unsafe { Box::from_raw(edit as *mut TrackEdit) });
}

struct OwnedTrackEdit {
    raw: *mut c_void,
    release: ReplayTrackEditFn,
}

impl Drop for OwnedTrackEdit {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { (self.release)(self.raw) };
            self.raw = std::ptr::null_mut();
        }
    }
}

fn record_track_in_history(
    ops: &HirariTrackEditHostOps,
    host: *mut c_void,
    label: &str,
    edit: *mut c_void,
) {
    let Some(read_state) = ops.undo_state else {
        unsafe { release_track_edit(edit) };
        return;
    };
    let state = unsafe { read_state(host) };
    if state.is_null() {
        unsafe { release_track_edit(edit) };
        return;
    }
    let owned = Arc::new(OwnedTrackEdit {
        raw: edit,
        release: release_track_edit,
    });
    let (undo_owned, redo_owned) = (owned.clone(), owned.clone());
    commit_closures(
        state,
        label.as_bytes(),
        Box::new(move || unsafe { replay_track_undo(undo_owned.raw) }),
        Box::new(move || unsafe { replay_track_redo(redo_owned.raw) }),
    );
}

/// Hands one lifecycle edit to the history. Without a native recorder the
/// entry goes into the engine's Rust history; with no history the track is
/// released instead of leaked.
fn record_track_edit(
    ops: &HirariTrackEditHostOps,
    host: *mut c_void,
    label: &str,
    edit: TrackEdit,
) {
    let edit = Box::into_raw(Box::new(edit));
    match ops.record_replay {
        Some(record) => {
            unsafe {
                record(
                    host,
                    label.as_ptr(),
                    label.len(),
                    edit.cast(),
                    replay_track_undo,
                    replay_track_redo,
                    release_track_edit,
                )
            };
        }
        None => record_track_in_history(ops, host, label, edit.cast()),
    }
}

fn sync_structural_changes(ops: &HirariTrackEditHostOps, host: *mut c_void) {
    if let Some(sync) = ops.sync_structural_changes {
        unsafe { sync(host) };
    }
}

/// Removes a captured track. The flags pick which of the two paths the engine
/// had: an undone `Add Track` drops the track's sidechain links, an undone
/// removal also clears its routes, dependencies, connections and feedback.
fn remove_captured(
    stores: &Stores,
    captured: &CapturedTrack,
    drop_sidechain_links: bool,
    drop_routing: bool,
) {
    {
        let _guard = stores.lock();
        if drop_sidechain_links {
            unsafe { hirari_sidechain_engine_remove_track(stores.sidechain, captured.id) };
        }
        if drop_routing {
            unsafe {
                hirari_sidechain_routes_erase_track(stores.routes, captured.id);
                hirari_routing_engine_remove_dependencies_for_node(stores.routing, captured.id);
            }
        }
        // The detach hands back a second owning handle and the edit already
        // holds one, so this side is released straight away — the engine's own
        // removal discarded the shared pointer it got back the same way.
        let detached = stores.detach(captured.id);
        stores.release(detached);
    }
    if drop_routing {
        discard_routing(stores, captured.id);
    }
}

/// Re-inserts a captured track and replays everything its removal captured.
fn restore_captured(stores: &Stores, captured: &CapturedTrack, stamp_generation: bool) {
    {
        let _guard = stores.lock();
        stores.restore_at(captured, stamp_generation);
        unsafe {
            hirari_sidechain_routes_restore(stores.routes, stores.routing, captured.sidechain_routes)
        };
    }
    // The links and the connections are replayed after the lock is released, as
    // the engine did: both stores take their own lock and the graph is already
    // published by then.
    unsafe { hirari_sidechain_engine_restore_links(stores.sidechain, captured.sidechain_links) };
    for connection in &captured.routing {
        unsafe {
            hirari_routing_engine_add_connection(
                stores.routing,
                connection.source_id,
                connection.destination_id,
                connection.gain,
                connection.send,
                connection.pre_fader,
            );
        }
    }
    for connection in &captured.feedback {
        unsafe {
            hirari_routing_engine_add_feedback(
                stores.routing,
                connection.source_id,
                connection.destination_id,
                connection.gain,
            );
        }
    }
}

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
fn chain_state(chain: *mut c_void, index: u32) -> Vec<u8> {
    let mut length = 0usize;
    let data = unsafe { crate::effect_chain_host::hirari_effect_chain_processor_state(chain, index, &mut length) };
    if data.is_null() || length == 0 {
        return Vec::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, length).to_vec() };
    unsafe { crate::effect_chain_host::hirari_effect_chain_buffer_free(data, length) };
    bytes
}

/// Reads one sandbox slot's path out of a chain.
fn chain_sandbox_path(chain: *mut c_void, slot: usize) -> Vec<u8> {
    let length =
        unsafe { crate::effect_chain_host::hirari_effect_chain_sandbox_slot_path(chain, slot, std::ptr::null_mut(), 0) };
    if length == 0 {
        return Vec::new();
    }
    let mut path = vec![0u8; length];
    let written = unsafe {
        crate::effect_chain_host::hirari_effect_chain_sandbox_slot_path(
            chain,
            slot,
            path.as_mut_ptr(),
            length,
        )
    };
    path.truncate(written.min(length));
    path
}

/// Copies the store state a duplicated track inherits.
///
/// The engine builds the new track and hands over its storages; the name with
/// the copy suffix, the fader, the mute and phase flags, the three automation
/// lists, the plugin lanes and the chain in order are then read and written
/// store-to-store here. A sandbox path the destination chain refuses is skipped
/// like the engine's own walk did, so one rejected worker never aborts the
/// duplicate.
///
/// # Safety
/// Every handle must be a live storage of its kind, and the two handle sets must
/// belong to tracks of the same kind so the chain rebuild selects the same DSP
/// variants.
#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub unsafe extern "C" fn hirari_track_clone_state(
    source_strip: *mut c_void,
    source_runtime: *mut c_void,
    source_automation: *mut c_void,
    source_chain: *mut c_void,
    source_name: *mut c_void,
    dest_strip: *mut c_void,
    dest_runtime: *mut c_void,
    dest_automation: *mut c_void,
    dest_chain: *mut c_void,
    dest_name: *mut c_void,
    dest_is_bus: bool,
) -> bool {
    if source_strip.is_null()
        || source_automation.is_null()
        || source_chain.is_null()
        || dest_strip.is_null()
        || dest_automation.is_null()
        || dest_chain.is_null()
    {
        return false;
    }
    // A name that cannot carry the suffix aborts the duplicate before any other
    // storage is touched.
    if !crate::track_name_store::copy_with_suffix(
        source_name as *const c_void,
        dest_name,
        b" Copy",
    ) {
        return false;
    }
    unsafe {
        crate::channel_strip::hirari_channel_strip_set_gain(
            dest_strip,
            crate::channel_strip::hirari_channel_strip_gain(source_strip),
        );
        crate::channel_strip::hirari_channel_strip_set_pan(
            dest_strip,
            crate::channel_strip::hirari_channel_strip_pan(source_strip),
        );
        crate::track_runtime::hirari_track_runtime_set_muted(
            dest_runtime,
            crate::track_runtime::hirari_track_runtime_is_muted(source_runtime) as u8,
        );
        crate::track_runtime::hirari_track_runtime_set_phase_invert(
            dest_runtime,
            crate::track_runtime::hirari_track_runtime_is_phase_inverted(source_runtime) as u8,
        );
        for kind in 0..3u32 {
            let mut count = 0usize;
            let points =
                crate::track_automation_store::hirari_track_automation_store_points(
                    source_automation, kind, &mut count,
                );
            crate::track_automation_store::hirari_track_automation_store_set_points(
                dest_automation, kind, points, count,
            );
        }
        let mut lane_count = 0usize;
        let lanes = crate::track_automation_store::hirari_track_automation_store_plugin_lane_views(
            source_automation,
            &mut lane_count,
        );
        if lane_count != 0 {
            crate::track_automation_store::hirari_track_automation_store_set_plugin_lanes(
                dest_automation, lanes, lane_count,
            );
        }
        let slot_count = crate::effect_chain_host::hirari_effect_chain_sandbox_slot_count(source_chain);
        let mut sandbox_slot = 0usize;
        for index in 0..crate::effect_chain_host::hirari_effect_chain_count(source_chain) {
            let kind = crate::effect_chain_host::hirari_effect_chain_node_kind(source_chain, index);
            let state = chain_state(source_chain, index);
            let bypassed = crate::effect_chain_host::hirari_effect_chain_bypass(source_chain, index);
            let added = if kind == crate::effect_chain_nodes::SANDBOX_KIND {
                let path_index = sandbox_slot;
                sandbox_slot += 1;
                if path_index >= slot_count {
                    false
                } else {
                    let path = chain_sandbox_path(source_chain, path_index);
                    !path.is_empty()
                        && crate::effect_chain_host::hirari_effect_chain_add_sandbox(
                            dest_chain,
                            path.as_ptr(),
                            path.len(),
                        )
                }
            } else {
                crate::effect_chain_host::hirari_effect_chain_add_builtin(
                    dest_chain, kind, dest_is_bus, false,
                )
            };
            if added && !state.is_empty() {
                let dest_index =
                    crate::effect_chain_host::hirari_effect_chain_count(dest_chain) - 1;
                crate::effect_chain_host::hirari_effect_chain_processor_state_restore(
                    dest_chain,
                    dest_index,
                    state.as_ptr(),
                    state.len(),
                );
                crate::effect_chain_host::hirari_effect_chain_set_bypass(
                    dest_chain,
                    dest_index,
                    bypassed,
                );
            }
        }
    }
    true
}

#[cfg(test)]
#[path = "track_edit_host_tests.rs"]
mod tests;

