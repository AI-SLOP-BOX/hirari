//! Engine track table: the live document graph.
//!
//! The engine used to keep `std::vector<std::shared_ptr<Track>>`, a deferred
//! destruction list and a double-buffered audio-thread snapshot in C++. The
//! tracks themselves are Rust now ([`crate::engine_track`]), so this module
//! holds their arcs outright: order, identity resolution, retirement and
//! lock-free publication all read a track's id, role, bus slot and latency
//! without calling back into native code.
//!
//! Lock discipline: every mutator and every control-thread reader runs while
//! the engine's tracks lock is held, exactly as the C++ container did. The
//! published snapshot is read lock-free by the realtime callback, so a
//! publication fills the buffer the callback is not reading and only then flips
//! the index.

use crate::bus_audio::MAX_BUSES;
use crate::engine_track::{Track, NO_BUS_ID, TRACK_TYPE_BUS};
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Matches `HirariUnifiedEngine::kMaxTracks`.
const MAX_ACTIVE_TRACKS: usize = 256;
/// Matches `HirariUnifiedEngine::kMaxTrackMap`.
const TRACK_MAP_LEN: usize = 2048;

/// One published generation of the graph the realtime callback traverses. The
/// fields are plain because the writer only touches the inactive buffer, then
/// releases the index that selects it.
struct Snapshot {
    active: Vec<*mut c_void>,
    track_map: Vec<*mut c_void>,
    count: u32,
    generation: u64,
}

impl Snapshot {
    fn new() -> Self {
        Self {
            active: vec![std::ptr::null_mut(); MAX_ACTIVE_TRACKS],
            track_map: vec![std::ptr::null_mut(); TRACK_MAP_LEN],
            count: 0,
            generation: 1,
        }
    }
}

pub struct EngineTrackTable {
    /// The graph, in order. Holding the arc is what runs the track's drop order
    /// once its last reference goes.
    owners: Vec<Arc<Track>>,
    /// The object pointer behind each arc, kept parallel to `owners` so a
    /// control-thread walk and the published snapshot need no dereference.
    raws: Vec<*mut c_void>,
    /// Retired tracks. A device callback may still hold their pointers, so the
    /// engine releases them at shutdown instead of at detach time.
    deferred: Vec<Arc<Track>>,
    snapshots: [Snapshot; 2],
    active_index: AtomicUsize,
}

impl EngineTrackTable {
    fn index_of(&self, track_id: u32) -> Option<usize> {
        self.owners.iter().position(|track| track.id() == track_id)
    }

    fn insert_owner(&mut self, index: usize, track: Arc<Track>) {
        let at = index.min(self.owners.len());
        let object = Arc::as_ptr(&track).cast_mut().cast();
        self.owners.insert(at, track);
        self.raws.insert(at, object);
    }

    /// Publishes the active list and the id map. `stamp` decides whether the
    /// supplied device generation travels with this publication: a structural
    /// edit that does not change the audio format keeps the previous value, so
    /// a command queued for the old generation is never applied to a new one.
    fn publish(&mut self, generation: u64, stamp: bool) {
        let current = self.active_index.load(Ordering::Acquire);
        let next = 1 - current;
        let generation = if stamp {
            generation
        } else {
            self.snapshots[current].generation
        };
        let buffer = &mut self.snapshots[next];
        buffer.active.fill(std::ptr::null_mut());
        buffer.track_map.fill(std::ptr::null_mut());
        buffer.count = 0;
        buffer.generation = generation;
        for (&track, owner) in self.raws.iter().zip(&self.owners).take(MAX_ACTIVE_TRACKS) {
            buffer.active[buffer.count as usize] = track;
            buffer.count += 1;
            let id = owner.id() as usize;
            if id < TRACK_MAP_LEN {
                buffer.track_map[id] = track;
            }
        }
        self.active_index.store(next, Ordering::Release);
    }

    /// The one control walk that has to be ordered against publication: mark
    /// the bus slots the graph actually occupies, rebuild the routing, publish
    /// this graph to the callback, then solve delay compensation and hand every
    /// track its offset. Tracks beyond the PDC id range keep whatever offset the
    /// solver never had for them, exactly as before.
    fn sync_structure(
        &mut self,
        bus_system: *mut c_void,
        routing: *mut c_void,
        pdc: *mut c_void,
        generation: u64,
    ) {
        let live = self.owners.clone();

        let mut active_buses = vec![false; MAX_BUSES];
        for track in &live {
            if track.kind() != TRACK_TYPE_BUS {
                continue;
            }
            let slot = track.bus_slot();
            if slot != NO_BUS_ID && (slot as usize) < active_buses.len() {
                active_buses[slot as usize] = true;
            }
        }
        unsafe {
            crate::bus_audio::hirari_bus_system_reconcile(
                bus_system,
                active_buses.as_ptr(),
                active_buses.len() as u32,
            )
        };
        unsafe { crate::routing_engine_core::hirari_routing_engine_build(routing) };
        self.publish(generation, true);

        let mut configs = Vec::with_capacity(live.len());
        let mut compensated = Vec::with_capacity(live.len());
        for track in &live {
            let id = track.id();
            if id as usize >= crate::native_pdc::MAX_TRACKS {
                continue;
            }
            configs.push(crate::native_pdc::PdcTrackConfig {
                id,
                latency_samples: track.latency_samples(),
            });
            compensated.push(track);
        }
        let mut offsets = vec![0u32; configs.len()];
        let solved =
            crate::native_pdc::sync_graph(pdc, routing, &configs, offsets.as_mut_slice());
        for (&track, offset) in compensated.iter().zip(offsets).take(solved) {
            track.apply_pdc_offset(offset);
        }
    }

    /// The project-load commit point, in the order the audio path needs: the
    /// outgoing graph loses its sidechain entries by id, every project-scoped
    /// manager is reset, then the old tracks are released because the
    /// replacement graph is already validated. The caller inserts the new tracks
    /// after this returns.
    fn reset_project(
        &mut self,
        rings: *mut c_void,
        pdc: *mut c_void,
        sidechain: *mut c_void,
        routing: *mut c_void,
        bus_system: *mut c_void,
        control_room: *mut c_void,
        sidechain_routes: *mut c_void,
    ) {
        let ids: Vec<u32> = self.owners.iter().map(|track| track.id()).collect();
        for track in ids {
            unsafe {
                crate::sidechain_manager::hirari_sidechain_engine_remove_track(sidechain, track)
            };
        }
        unsafe {
            crate::engine_rings::hirari_engine_command_ring_clear(rings.cast());
            crate::native_pdc::hirari_native_pdc_reset_for_project(pdc.cast());
            crate::sidechain_manager::hirari_sidechain_engine_reset(sidechain.cast());
            crate::routing_engine_core::hirari_routing_engine_reset(routing);
            crate::bus_audio::hirari_bus_system_reset(bus_system);
            crate::control_room_runtime::hirari_control_room_reset(control_room);
            crate::sidechain_route_store::hirari_sidechain_routes_clear(sidechain_routes.cast());
        }
        self.owners.clear();
        self.raws.clear();
    }
}

impl Drop for EngineTrackTable {
    fn drop(&mut self) {
        self.owners.clear();
        self.raws.clear();
        self.deferred.clear();
    }
}

/// An owning handle for one track, released by [`hirari_track_owner_release`].
fn owner_handle(track: Arc<Track>) -> *mut c_void {
    Box::into_raw(Box::new(track)).cast()
}

/// # Safety
/// The returned handle must be passed to the other `hirari_engine_track_*`
/// entry points, or be null.
#[no_mangle]
pub extern "C" fn hirari_engine_track_table_create() -> *mut c_void {
    let table = EngineTrackTable {
        owners: Vec::new(),
        raws: Vec::new(),
        deferred: Vec::new(),
        snapshots: [Snapshot::new(), Snapshot::new()],
        active_index: AtomicUsize::new(0),
    };
    Box::into_raw(Box::new(table)).cast()
}

/// # Safety
/// `table` must be null or a pointer from [`hirari_engine_track_table_create`],
/// and no track pointer handed to the realtime callback may still be in use.
/// Dropping the table releases every track it owns.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_track_table_destroy(table: *mut c_void) {
    if !table.is_null() {
        drop(Box::from_raw(table.cast::<EngineTrackTable>()));
    }
}

fn as_table<'a>(handle: *const c_void) -> Option<&'a EngineTrackTable> {
    unsafe { (handle as *mut EngineTrackTable).as_ref() }
}

fn as_table_mut<'a>(handle: *mut c_void) -> Option<&'a mut EngineTrackTable> {
    unsafe { handle.cast::<EngineTrackTable>().as_mut() }
}

/// Number of live tracks.
///
/// # Safety
/// `table` must be null or a pointer from [`hirari_engine_track_table_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_track_count(table: *const c_void) -> usize {
    as_table(table).map_or(0, |table| table.owners.len())
}

/// The live track object list in graph order. The pointer is the vector's
/// buffer, so it stays valid only while the engine tracks lock is held and no
/// mutator runs.
///
/// # Safety
/// `table` must be null or a pointer from [`hirari_engine_track_table_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_track_ptrs(table: *const c_void) -> *const *mut c_void {
    as_table(table).map_or_else(std::ptr::null, |table| table.raws.as_ptr())
}

/// Resolves a track id to its object pointer, or null when the id is not in the
/// graph.
///
/// # Safety
/// `table` must be null or a pointer from [`hirari_engine_track_table_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_track_find(
    table: *const c_void,
    track_id: u32,
) -> *mut c_void {
    let Some(table) = as_table(table) else {
        return std::ptr::null_mut();
    };
    table
        .index_of(track_id)
        .map_or(std::ptr::null_mut(), |index| table.raws[index])
}

/// Position of a track id in graph order, or `usize::MAX` when the id is gone.
///
/// # Safety
/// `table` must be null or a pointer from [`hirari_engine_track_table_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_track_index_of(
    table: *const c_void,
    track_id: u32,
) -> usize {
    let Some(table) = as_table(table) else {
        return usize::MAX;
    };
    table.index_of(track_id).unwrap_or(usize::MAX)
}

/// A fresh owning handle for the track with this id, or null. The caller
/// releases it with [`hirari_track_owner_release`].
///
/// # Safety
/// `table` must be null or a pointer from [`hirari_engine_track_table_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_track_retain(
    table: *const c_void,
    track_id: u32,
) -> *mut c_void {
    let Some(table) = as_table(table) else {
        return std::ptr::null_mut();
    };
    table
        .index_of(track_id)
        .map_or(std::ptr::null_mut(), |index| {
            owner_handle(table.owners[index].clone())
        })
}

/// Takes over `owner`, the owning handle [`hirari_track_new`] returned, at
/// `index`, clamped to the end of the graph, so an appending caller passes
/// `usize::MAX`. A null owner is rejected so a failed construction cannot leave
/// a hole.
///
/// # Safety
/// `table` must come from [`hirari_engine_track_table_create`] and `owner` must
/// be an owning handle nobody else frees.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_track_insert(
    table: *mut c_void,
    index: usize,
    owner: *mut c_void,
) -> bool {
    if owner.is_null() {
        return false;
    }
    let Some(table) = as_table_mut(table) else {
        return false;
    };
    let track = unsafe { *Box::from_raw(owner.cast::<Arc<Track>>()) };
    table.insert_owner(index, track);
    true
}

/// Removes the track from the graph and keeps it alive in the deferred list.
/// Returns a handle owned by the caller; the deferred list keeps its own so the
/// object survives until the engine releases it. Null when the id is gone.
///
/// # Safety
/// `table` must come from [`hirari_engine_track_table_create`] and the engine
/// tracks lock must be held. The caller releases the returned handle with
/// [`hirari_track_owner_release`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_track_detach(
    table: *mut c_void,
    track_id: u32,
) -> *mut c_void {
    let Some(table) = as_table_mut(table) else {
        return std::ptr::null_mut();
    };
    let Some(index) = table.index_of(track_id) else {
        return std::ptr::null_mut();
    };
    let track = table.owners.remove(index);
    table.raws.remove(index);
    let caller = owner_handle(track.clone());
    table.deferred.push(track);
    caller
}

/// Moves the whole live graph into the deferred list, which is what starting a
/// new project does: the old tracks must outlive any callback that still holds
/// their pointers.
///
/// # Safety
/// `table` must come from [`hirari_engine_track_table_create`] and the engine
/// tracks lock must be held.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_track_defer_all(table: *mut c_void) {
    if let Some(table) = as_table_mut(table) {
        table.deferred.append(&mut table.owners);
        table.raws.clear();
    }
}

/// # Safety
/// `table` must come from [`hirari_engine_track_table_create`] and the engine
/// tracks lock must be held.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_track_publish(
    table: *mut c_void,
    generation: u64,
    stamp: bool,
) {
    if let Some(table) = as_table_mut(table) {
        table.publish(generation, stamp);
    }
}

/// Runs the engine's structural sync over the graph: see
/// [`EngineTrackTable::sync_structure`].
///
/// # Safety
/// `table` must come from [`hirari_engine_track_table_create`] and the engine
/// tracks lock must be held. `bus_system`, `routing` and `pdc` must be the
/// handles the engine created for them or null.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_sync_structure(
    table: *mut c_void,
    bus_system: *mut c_void,
    routing: *mut c_void,
    pdc: *mut c_void,
    generation: u64,
) {
    if let Some(table) = as_table_mut(table) {
        table.sync_structure(bus_system, routing, pdc, generation);
    }
}

/// Runs the engine's project-load commit point; see
/// [`EngineTrackTable::reset_project`].
///
/// # Safety
/// `table` must come from [`hirari_engine_track_table_create`] and the engine
/// tracks lock must be held. Every other argument must be the handle the engine
/// created for it, or null.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_reset_project(
    table: *mut c_void,
    rings: *mut c_void,
    pdc: *mut c_void,
    sidechain: *mut c_void,
    routing: *mut c_void,
    bus_system: *mut c_void,
    control_room: *mut c_void,
    sidechain_routes: *mut c_void,
) {
    if let Some(table) = as_table_mut(table) {
        table.reset_project(
            rings,
            pdc,
            sidechain,
            routing,
            bus_system,
            control_room,
            sidechain_routes,
        );
    }
}

/// Mirrors the C++ view the realtime callback walks: the published buffers keep
/// their addresses for the table's lifetime, so only their contents change and
/// only under a publication.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HirariTrackSnapshot {
    pub active_tracks: *const *mut c_void,
    pub track_map: *const *mut c_void,
    pub track_count: u32,
    pub audio_config_generation: u64,
}

/// # Safety
/// `table` must be null or a pointer from [`hirari_engine_track_table_create`]
/// and `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_track_snapshot(
    table: *const c_void,
    out: *mut HirariTrackSnapshot,
) {
    if out.is_null() {
        return;
    }
    let Some(table) = as_table(table) else {
        *out = HirariTrackSnapshot::default();
        return;
    };
    let snapshot = &table.snapshots[table.active_index.load(Ordering::Acquire)];
    *out = HirariTrackSnapshot {
        active_tracks: snapshot.active.as_ptr(),
        track_map: snapshot.track_map.as_ptr(),
        track_count: snapshot.count,
        audio_config_generation: snapshot.generation,
    };
}

#[cfg(test)]
#[path = "engine_track_table_tests.rs"]
mod tests;

