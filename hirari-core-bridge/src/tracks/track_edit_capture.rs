//! Capture/undo machinery for the track-edit lifecycle, split out of
//! track_edit_host.rs.
use super::*;

pub(crate) struct Stores {
    pub(crate) host: *mut c_void,
    pub(crate) block: *mut c_void,
    pub(crate) table: *mut c_void,
    pub(crate) revision: *mut c_void,
    pub(crate) bus_system: *mut c_void,
    pub(crate) sidechain: *mut c_void,
    pub(crate) routes: *mut c_void,
    pub(crate) routing: *mut c_void,
}

impl Stores {
    pub(crate) fn new(ops: &HirariTrackEditHostOps, host: *mut c_void) -> Option<Self> {
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

    pub(crate) fn lock(&self) -> Option<track_session::GraphLock> {
        track_session::GraphLock::acquire(self.host)
    }

    pub(crate) fn find(&self, track_id: u32) -> *mut c_void {
        unsafe { hirari_engine_track_find(self.table, track_id) }
    }

    /// Releases one owning handle this walk no longer needs.
    pub(crate) fn release(&self, owner: *mut c_void) {
        if !owner.is_null() {
            unsafe { hirari_track_owner_release(owner) };
        }
    }

    /// Republishes the graph at the engine block's current generation. `stamp`
    /// decides whether that generation travels with the publication: an edit
    /// that did not change the audio format keeps the previous value, so a
    /// command queued for the old generation is never applied to a new one.
    pub(crate) fn publish(&self, stamp: bool) {
        unsafe {
            hirari_engine_track_publish(self.table, hirari_engine_block_generation(self.block), stamp)
        };
    }

    /// Builds a track of this kind and id, names it, and brings it up at the
    /// device format the graph is prepared for. A bus takes its slot from the
    /// engine's bus system first: the structural walk can only route to a slot
    /// the system actually hands out, so an unresolvable bus stays slotless.
    pub(crate) fn build(&self, kind: u32, id: u32, name: &str) -> (*mut c_void, *mut c_void) {
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
    pub(crate) fn append(&self, owner: *mut c_void) -> usize {
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
    pub(crate) fn detach(&self, track_id: u32) -> *mut c_void {
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
    pub(crate) fn restore_at(&self, captured: &CapturedTrack, stamp: bool) {
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
pub(crate) struct TrackOwner {
    pub(crate) owner: *mut c_void,
}

impl TrackOwner {
    /// Hands the handle to a capture that takes over the release.
    pub(crate) fn into_captured(self, track: *mut c_void, index: usize, id: u32) -> CapturedTrack {
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
pub(crate) fn take_routing_records(stores: &Stores, track_id: u32) -> Vec<RoutingConnectionRecord> {
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
pub(crate) fn take_feedback_records(stores: &Stores, track_id: u32) -> Vec<FeedbackConnection> {
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
pub(crate) fn discard_routing(stores: &Stores, track_id: u32) {
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
    pub(crate) owner: *mut c_void,
    pub(crate) track: *mut c_void,
    pub(crate) index: usize,
    pub(crate) id: u32,
    pub(crate) sidechain_links: *mut c_void,
    pub(crate) sidechain_routes: *mut c_void,
    pub(crate) routing: Vec<RoutingConnectionRecord>,
    pub(crate) feedback: Vec<FeedbackConnection>,
}

impl CapturedTrack {
    /// A track the edit just built: nothing was taken out of the graph with it,
    /// so undo only has to remove it again.
    pub(crate) fn created(owner: *mut c_void, track: *mut c_void, index: usize, id: u32) -> Self {
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

pub(crate) enum TrackEditKind {
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
pub(crate) struct TrackEdit {
    pub(crate) ops: HirariTrackEditHostOps,
    pub(crate) host: *mut c_void,
    pub(crate) kind: TrackEditKind,
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

pub(crate) struct OwnedTrackEdit {
    pub(crate) raw: *mut c_void,
    pub(crate) release: ReplayTrackEditFn,
}

impl Drop for OwnedTrackEdit {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { (self.release)(self.raw) };
            self.raw = std::ptr::null_mut();
        }
    }
}

pub(crate) fn record_track_in_history(
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
pub(crate) fn record_track_edit(
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

pub(crate) fn sync_structural_changes(ops: &HirariTrackEditHostOps, host: *mut c_void) {
    if let Some(sync) = ops.sync_structural_changes {
        unsafe { sync(host) };
    }
}

/// Removes a captured track. The flags pick which of the two paths the engine
/// had: an undone `Add Track` drops the track's sidechain links, an undone
/// removal also clears its routes, dependencies, connections and feedback.
pub(crate) fn remove_captured(
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
pub(crate) fn restore_captured(stores: &Stores, captured: &CapturedTrack, stamp_generation: bool) {
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
