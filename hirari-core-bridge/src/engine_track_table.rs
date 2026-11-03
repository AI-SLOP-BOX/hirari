//! Engine track table: the live document graph.
//!
//! The engine used to keep `std::vector<std::shared_ptr<Track>>`, a deferred
//! destruction list and a double-buffered audio-thread snapshot in C++. Order,
//! identity resolution, retirement and lock-free publication now live here. The
//! `Track` objects themselves stay C++ because their processing is virtual, and
//! this module only ever moves the opaque handles around.
//!
//! Lock discipline: every mutator and every control-thread reader runs while
//! the engine's tracks lock is held, exactly as the C++ container did. The
//! published snapshot is read lock-free by the realtime callback, so a
//! publication fills the buffer the callback is not reading and only then flips
//! the index.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Mirrors `HirariEngineTrackOps` in `src/core/rust_ffi.hpp`.
#[repr(C)]
pub struct HirariEngineTrackOps {
    pub id: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    pub clone_owner: Option<unsafe extern "C" fn(*mut c_void) -> *mut c_void>,
    pub destroy_owner: Option<unsafe extern "C" fn(*mut c_void)>,
    /// `Track::Type` as its numeric value; 3 is a bus.
    pub track_type: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    /// The bus slot a bus track occupies, or [`NO_BUS_ID`] for every other kind.
    pub bus_id: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    /// The track's own reported latency, the input to the PDC solve.
    pub latency: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    /// Applies a solved PDC offset to one track.
    pub set_pdc_offset: Option<unsafe extern "C" fn(*mut c_void, u32)>,
}

/// A track that is not a bus has no bus slot.
pub const NO_BUS_ID: u32 = u32::MAX;

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
    ops: *const HirariEngineTrackOps,
    /// `std::shared_ptr<Track>*` handles owned by this table, in graph order.
    owners: Vec<*mut c_void>,
    /// The `Track*` behind each handle, kept parallel to `owners` so a
    /// control-thread walk needs no per-entry call into C++.
    raws: Vec<*mut c_void>,
    /// Retired tracks. A device callback may still hold their pointers, so the
    /// engine destroys them at shutdown instead of at detach time.
    deferred: Vec<*mut c_void>,
    snapshots: [Snapshot; 2],
    active_index: AtomicUsize,
}

impl EngineTrackTable {
    fn id(&self, track: *mut c_void) -> u32 {
        let op = unsafe { (*self.ops).id }.expect("track id op");
        unsafe { op(track) }
    }

    fn clone_owner(&self, owner: *mut c_void) -> *mut c_void {
        let op = unsafe { (*self.ops).clone_owner }.expect("track clone op");
        unsafe { op(owner) }
    }

    /// The release op read out of the table so a drain loop can destroy handles
    /// while it still holds the owning vector.
    fn destroy_op(&self) -> unsafe extern "C" fn(*mut c_void) {
        unsafe { (*self.ops).destroy_owner }.expect("track destroy op")
    }

    fn index_of(&self, track_id: u32) -> Option<usize> {
        self.raws
            .iter()
            .position(|&track| !track.is_null() && self.id(track) == track_id)
    }

    fn destroy_owners(&mut self) {
        let destroy = self.destroy_op();
        for owner in self.owners.drain(..) {
            unsafe { destroy(owner) };
        }
        self.raws.clear();
    }

    fn destroy_deferred(&mut self) {
        let destroy = self.destroy_op();
        for owner in self.deferred.drain(..) {
            unsafe { destroy(owner) };
        }
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
        let id = unsafe { (*self.ops).id }.expect("track id op");
        let raws = &self.raws;
        let buffer = &mut self.snapshots[next];
        buffer.active.fill(std::ptr::null_mut());
        buffer.track_map.fill(std::ptr::null_mut());
        buffer.count = 0;
        buffer.generation = generation;
        for &track in raws
            .iter()
            .filter(|track| !track.is_null())
            .take(MAX_ACTIVE_TRACKS)
        {
            buffer.active[buffer.count as usize] = track;
            buffer.count += 1;
            let id = unsafe { id(track) };
            if (id as usize) < TRACK_MAP_LEN {
                buffer.track_map[id as usize] = track;
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
        let (id, track_type, bus_id, latency, set_offset) = unsafe {
            (
                (*self.ops).id,
                (*self.ops).track_type,
                (*self.ops).bus_id,
                (*self.ops).latency,
                (*self.ops).set_pdc_offset,
            )
        };
        let (id, track_type, bus_id, latency, set_offset) = (
            id.expect("track id op"),
            track_type.expect("track type op"),
            bus_id.expect("track bus id op"),
            latency.expect("track latency op"),
            set_offset.expect("track pdc offset op"),
        );
        let live: Vec<*mut c_void> = self
            .raws
            .iter()
            .copied()
            .filter(|track| !track.is_null())
            .collect();

        let mut active_buses = vec![false; crate::bus_audio::MAX_BUSES];
        for &track in &live {
            if unsafe { track_type(track) } != crate::track_edit_host::TRACK_TYPE_BUS {
                continue;
            }
            let slot = unsafe { bus_id(track) };
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
        for &track in &live {
            let track_id = unsafe { id(track) };
            if track_id as usize >= crate::native_pdc::MAX_TRACKS {
                continue;
            }
            configs.push(crate::native_pdc::PdcTrackConfig {
                id: track_id,
                latency_samples: unsafe { latency(track) },
            });
            compensated.push(track);
        }
        let mut offsets = vec![0u32; configs.len()];
        let solved =
            crate::native_pdc::sync_graph(pdc, routing, &configs, offsets.as_mut_slice());
        for (&track, offset) in compensated.iter().zip(offsets).take(solved) {
            unsafe { set_offset(track, offset) };
        }
    }

    /// The project-load commit point, in the order the audio path needs: the
    /// outgoing graph loses its sidechain entries by id, every project-scoped
    /// manager is reset, then the old tracks are destroyed because the
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
        let ids: Vec<u32> = self
            .raws
            .iter()
            .filter(|track| !track.is_null())
            .map(|&track| self.id(track))
            .collect();
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
        self.destroy_owners();
    }
}

impl Drop for EngineTrackTable {
    fn drop(&mut self) {
        self.destroy_owners();
        self.destroy_deferred();
    }
}

/// # Safety
/// `ops` must point at a table that outlives the returned engine track table.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_track_table_create(
    ops: *const HirariEngineTrackOps,
) -> *mut c_void {
    if ops.is_null() {
        return std::ptr::null_mut();
    }
    let table = EngineTrackTable {
        ops,
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
/// Dropping the table destroys every handle it owns.
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
    as_table(table).map_or(0, |table| table.raws.len())
}

/// The live `Track*` list in graph order. The pointer is the vector's buffer,
/// so it stays valid only while the engine tracks lock is held and no mutator
/// runs.
///
/// # Safety
/// `table` must be null or a pointer from [`hirari_engine_track_table_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_track_ptrs(table: *const c_void) -> *const *mut c_void {
    as_table(table).map_or_else(std::ptr::null, |table| table.raws.as_ptr())
}

/// Resolves a track id to its `Track*`, or null when the id is not in the graph.
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

/// Clones the owning handle of one track, or returns null when the id is gone.
///
/// # Safety
/// `table` must be null or a pointer from [`hirari_engine_track_table_create`].
/// The caller owns the returned handle and releases it by deleting the C++
/// `std::shared_ptr<Track>` it points at.
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
            table.clone_owner(table.owners[index])
        })
}

/// Takes over `owner` at `index`, clamped to the end of the graph, so an
/// appending caller passes `usize::MAX`. `track` must be the object `owner`
/// points at; a null owner is rejected so a failed construction cannot leave a
/// hole.
///
/// # Safety
/// `table` must come from [`hirari_engine_track_table_create`], `owner` must be
/// a heap handle nobody else frees, and `track` must be the object inside it.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_track_insert(
    table: *mut c_void,
    index: usize,
    owner: *mut c_void,
    track: *mut c_void,
) -> bool {
    if owner.is_null() {
        return false;
    }
    as_table_mut(table).is_some_and(|table| {
        let at = index.min(table.owners.len());
        table.owners.insert(at, owner);
        table.raws.insert(at, track);
        true
    })
}

/// Removes the track from the graph and keeps it alive in the deferred list.
/// Returns a handle owned by the caller; the deferred list keeps its own so the
/// object survives until the engine releases it. Null when the id is gone.
///
/// # Safety
/// `table` must come from [`hirari_engine_track_table_create`] and the engine
/// tracks lock must be held. The caller releases the returned handle by
/// deleting the C++ `std::shared_ptr<Track>` it points at.
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
    let owner = table.owners.remove(index);
    table.raws.remove(index);
    table.deferred.push(owner);
    table.clone_owner(owner)
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
        table.reset_project(rings, pdc, sidechain, routing, bus_system, control_room, sidechain_routes);
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
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// Stand-in for the `Track`: destroyed when its last handle is released.
    struct Shared {
        id: u32,
        refs: usize,
    }

    /// Stand-in for the heap `std::shared_ptr<Track>` the table owns.
    struct Owner {
        shared: *mut Shared,
    }

    static HANDLES: AtomicUsize = AtomicUsize::new(0);
    static DESTROYED: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    /// The `(Track*, type, bus slot, latency)` the structural ops read for a
    /// stand-in pointer, and the offsets `set_pdc_offset` was handed.
    static FACTS: Mutex<Vec<(usize, u32, u32, u32)>> = Mutex::new(Vec::new());
    static PDC_OFFSETS: Mutex<Vec<(u32, u32)>> = Mutex::new(Vec::new());
    /// The two counters are process-global, so the tests take this lock instead
    /// of running concurrently and interleaving their accounting.
    static SERIALIZED: Mutex<()> = Mutex::new(());

    /// Takes the lock and clears the logs, so a test only ever sees the tracks
    /// it created itself.
    fn serialize() -> MutexGuard<'static, ()> {
        let guard = SERIALIZED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        DESTROYED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        FACTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        PDC_OFFSETS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        guard
    }

    unsafe extern "C" fn raw_id(raw: *mut c_void) -> u32 {
        raw as usize as u32
    }

    unsafe extern "C" fn clone_owner(owner: *mut c_void) -> *mut c_void {
        let owner = unsafe { &*owner.cast::<Owner>() };
        unsafe { (*owner.shared).refs += 1 };
        HANDLES.fetch_add(1, Ordering::Relaxed);
        Box::into_raw(Box::new(Owner {
            shared: owner.shared,
        }))
        .cast()
    }

    unsafe extern "C" fn destroy_owner(owner: *mut c_void) {
        let owner = unsafe { Box::from_raw(owner.cast::<Owner>()) };
        HANDLES.fetch_sub(1, Ordering::Relaxed);
        let shared = unsafe { &mut *owner.shared };
        shared.refs -= 1;
        if shared.refs == 0 {
            DESTROYED
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(shared.id);
            drop(unsafe { Box::from_raw(owner.shared) });
        }
    }

    /// Reads one structural fact for a stand-in pointer; an unregistered track
    /// answers as an audio track with no latency.
    fn fact(raw: *mut c_void, select: impl Fn(&(usize, u32, u32, u32)) -> u32) -> u32 {
        let facts = FACTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        facts
            .iter()
            .find(|(track, ..)| *track == raw as usize)
            .map_or(0, select)
    }

    fn set_facts(id: u32, track_type: u32, bus_id: u32, latency: u32) {
        FACTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((raw(id) as usize, track_type, bus_id, latency));
    }

    fn pdc_offsets() -> Vec<(u32, u32)> {
        let mut offsets = PDC_OFFSETS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut offsets)
    }

    unsafe extern "C" fn track_type(raw: *mut c_void) -> u32 {
        fact(raw, |&(_, track_type, _, _)| track_type)
    }

    unsafe extern "C" fn bus_id(raw: *mut c_void) -> u32 {
        fact(raw, |&(_, _, slot, _)| slot)
    }

    unsafe extern "C" fn latency(raw: *mut c_void) -> u32 {
        fact(raw, |&(_, _, _, latency)| latency)
    }

    unsafe extern "C" fn set_pdc_offset(raw: *mut c_void, samples: u32) {
        PDC_OFFSETS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((unsafe { raw_id(raw) }, samples));
    }

    static OPS: HirariEngineTrackOps = HirariEngineTrackOps {
        id: Some(raw_id),
        clone_owner: Some(clone_owner),
        destroy_owner: Some(destroy_owner),
        track_type: Some(track_type),
        bus_id: Some(bus_id),
        latency: Some(latency),
        set_pdc_offset: Some(set_pdc_offset),
    };

    /// The raw `Track*` the table publishes is this id, so a walk needs no
    /// allocation and a test can read identity straight back out of a pointer.
    fn raw(id: u32) -> *mut c_void {
        id as usize as *mut c_void
    }

    fn new_handle(id: u32) -> *mut c_void {
        let shared = Box::into_raw(Box::new(Shared { id, refs: 1 }));
        HANDLES.fetch_add(1, Ordering::Relaxed);
        Box::into_raw(Box::new(Owner { shared })).cast()
    }

    fn owner_id(owner: *mut c_void) -> u32 {
        unsafe { (*(*owner.cast::<Owner>()).shared).id }
    }

    fn release_handle(owner: *mut c_void) {
        unsafe { destroy_owner(owner) };
    }

    fn live_handles() -> usize {
        HANDLES.load(Ordering::Relaxed)
    }

    fn destroyed_tracks() -> Vec<u32> {
        let mut log = DESTROYED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut log)
    }

    struct Table(*mut c_void);

    impl Table {
        fn new() -> Self {
            Table(unsafe { hirari_engine_track_table_create(&OPS) })
        }
        fn get(&self) -> *mut c_void {
            self.0
        }
        fn push(&self, id: u32) {
            assert!(unsafe {
                hirari_engine_track_insert(self.get(), usize::MAX, new_handle(id), raw(id))
            });
        }
        fn publish(&self, generation: u64, stamp: bool) {
            unsafe { hirari_engine_track_publish(self.get(), generation, stamp) };
        }
        fn snapshot(&self) -> HirariTrackSnapshot {
            let mut out = HirariTrackSnapshot::default();
            unsafe { hirari_engine_track_snapshot(self.get(), &mut out) };
            assert!(!out.active_tracks.is_null() && !out.track_map.is_null());
            out
        }
    }

    impl Drop for Table {
        fn drop(&mut self) {
            unsafe { hirari_engine_track_table_destroy(self.get()) };
        }
    }

    fn published_ids(snapshot: &HirariTrackSnapshot) -> Vec<u32> {
        (0..snapshot.track_count as usize)
            .map(|index| unsafe { *snapshot.active_tracks.add(index) as usize as u32 })
            .collect()
    }

    fn mapped(snapshot: &HirariTrackSnapshot, id: u32) -> bool {
        !unsafe { *snapshot.track_map.add(id as usize) }.is_null()
    }

    #[test]
    fn graph_keeps_push_order_and_resolves_ids() {
        let _guard = serialize();
        let table = Table::new();
        table.push(7);
        table.push(3);
        assert_eq!(unsafe { hirari_engine_track_count(table.get()) }, 2);
        assert_eq!(
            unsafe { std::slice::from_raw_parts(hirari_engine_track_ptrs(table.get()), 2) }
                .to_vec(),
            vec![raw(7), raw(3)]
        );
        assert_eq!(unsafe { hirari_engine_track_find(table.get(), 3) }, raw(3));
        assert_eq!(unsafe { hirari_engine_track_index_of(table.get(), 3) }, 1);
        assert!(unsafe { hirari_engine_track_find(table.get(), 9) }.is_null());
        assert_eq!(
            unsafe { hirari_engine_track_index_of(table.get(), 9) },
            usize::MAX
        );
    }

    #[test]
    fn insert_clamps_the_index_and_rejects_a_null_owner() {
        let _guard = serialize();
        let table = Table::new();
        table.push(1);
        table.push(2);
        assert!(!unsafe {
            hirari_engine_track_insert(table.get(), usize::MAX, std::ptr::null_mut(), raw(3))
        });
        assert!(unsafe { hirari_engine_track_insert(table.get(), 99, new_handle(3), raw(3)) });
        assert!(unsafe { hirari_engine_track_insert(table.get(), 0, new_handle(4), raw(4)) });
        assert_eq!(
            unsafe { std::slice::from_raw_parts(hirari_engine_track_ptrs(table.get()), 4) }
                .to_vec(),
            vec![raw(4), raw(1), raw(2), raw(3)]
        );
        assert_eq!(unsafe { hirari_engine_track_index_of(table.get(), 3) }, 3);
    }

    #[test]
    fn detach_returns_a_second_reference_and_defers_the_track() {
        let _guard = serialize();
        let table = Table::new();
        table.push(11);
        table.push(12);
        let before = live_handles();
        let detached = unsafe { hirari_engine_track_detach(table.get(), 11) };
        assert!(!detached.is_null());
        assert_eq!(owner_id(detached), 11);
        assert_eq!(unsafe { hirari_engine_track_count(table.get()) }, 1);
        assert_eq!(live_handles(), before + 1);
        release_handle(detached);
        // The caller's reference is gone but the deferred list still holds the
        // track, so no audio-thread pointer can dangle.
        assert_eq!(live_handles(), before);
        assert!(destroyed_tracks().is_empty());
        assert!(unsafe { hirari_engine_track_detach(table.get(), 99) }.is_null());
        // Dropping the table releases the live handles first, then the deferred
        // ones, so every track the callback may have seen is destroyed.
        drop(table);
        assert_eq!(destroyed_tracks(), vec![12, 11]);
    }

    #[test]
    fn publish_stamps_the_generation_and_detach_clears_the_id_map() {
        let _guard = serialize();
        let table = Table::new();
        table.push(21);
        table.push(22);
        table.publish(7, true);
        let snapshot = table.snapshot();
        assert_eq!(published_ids(&snapshot), vec![21, 22]);
        assert_eq!(snapshot.audio_config_generation, 7);
        assert!(mapped(&snapshot, 21) && mapped(&snapshot, 22));
        release_handle(unsafe { hirari_engine_track_detach(table.get(), 21) });
        table.publish(0, false);
        let snapshot = table.snapshot();
        assert_eq!(published_ids(&snapshot), vec![22]);
        assert_eq!(snapshot.audio_config_generation, 7);
        assert!(!mapped(&snapshot, 21));
    }

    #[test]
    fn publish_without_a_stamp_keeps_the_previous_generation() {
        let _guard = serialize();
        let table = Table::new();
        table.push(1);
        table.publish(3, true);
        assert_eq!(table.snapshot().audio_config_generation, 3);
        table.push(2);
        table.publish(99, false);
        assert_eq!(table.snapshot().audio_config_generation, 3);
        assert_eq!(table.snapshot().track_count, 2);
        table.publish(4, true);
        assert_eq!(table.snapshot().audio_config_generation, 4);
    }

    #[test]
    fn publication_writes_the_buffer_the_callback_is_not_reading() {
        let _guard = serialize();
        let table = Table::new();
        table.push(31);
        table.push(32);
        table.publish(1, true);
        let held = table.snapshot();
        assert_eq!(published_ids(&held), vec![31, 32]);
        release_handle(unsafe { hirari_engine_track_detach(table.get(), 31) });
        release_handle(unsafe { hirari_engine_track_detach(table.get(), 32) });
        table.publish(2, true);
        assert_eq!(published_ids(&table.snapshot()), Vec::<u32>::new());
        // The realtime callback may still be traversing the buffer it read.
        assert_eq!(published_ids(&held), vec![31, 32]);
    }

    #[test]
    fn publication_clamps_to_the_realtime_capacity() {
        let _guard = serialize();
        let table = Table::new();
        for id in 1..=(MAX_ACTIVE_TRACKS as u32 + 2) {
            table.push(id);
        }
        table.publish(1, true);
        let snapshot = table.snapshot();
        assert_eq!(snapshot.track_count as usize, MAX_ACTIVE_TRACKS);
        assert_eq!(published_ids(&snapshot).len(), MAX_ACTIVE_TRACKS);
        assert!(mapped(&snapshot, MAX_ACTIVE_TRACKS as u32));
        assert!(!mapped(&snapshot, MAX_ACTIVE_TRACKS as u32 + 1));
        assert!(!mapped(&snapshot, MAX_ACTIVE_TRACKS as u32 + 2));
    }

    #[test]
    fn ids_beyond_the_map_capacity_are_published_but_not_mapped() {
        let _guard = serialize();
        let table = Table::new();
        let id = TRACK_MAP_LEN as u32 + 5;
        table.push(id);
        table.push(9);
        table.publish(1, true);
        let snapshot = table.snapshot();
        assert_eq!(published_ids(&snapshot), vec![id, 9]);
        assert!(mapped(&snapshot, 9));
    }

    #[test]
    fn retain_answers_a_second_reference_for_the_graph() {
        let _guard = serialize();
        let table = Table::new();
        table.push(41);
        table.push(42);
        let before = live_handles();
        let one = unsafe { hirari_engine_track_retain(table.get(), 42) };
        assert_eq!(owner_id(one), 42);
        assert_eq!(live_handles(), before + 1);
        release_handle(one);
        assert!(unsafe { hirari_engine_track_retain(table.get(), 99) }.is_null());
        assert_eq!(live_handles(), before);
    }

    #[test]
    fn defer_all_holds_the_graph_until_the_table_dies() {
        let _guard = serialize();
        let table = Table::new();
        table.push(51);
        table.push(52);
        unsafe { hirari_engine_track_defer_all(table.get()) };
        assert_eq!(unsafe { hirari_engine_track_count(table.get()) }, 0);
        assert!(destroyed_tracks().is_empty());
        assert_eq!(live_handles(), 2);
        drop(table);
        assert_eq!(destroyed_tracks(), vec![51, 52]);
        assert_eq!(live_handles(), 0);
    }

    #[test]
    fn reset_project_destroys_the_graph_and_resets_the_managers() {
        let _guard = serialize();
        let table = Table::new();
        set_facts(2, crate::track_edit_host::TRACK_TYPE_BUS, 5, 0);
        table.push(2);
        let buses = crate::bus_audio::hirari_bus_system_create();
        let null = std::ptr::null_mut();
        unsafe { hirari_engine_sync_structure(table.get(), buses, null, null, 1) };
        assert!(unsafe { crate::bus_audio::hirari_bus_system_is_present(buses, 5) });
        unsafe {
            hirari_engine_reset_project(table.get(), null, null, null, null, buses, null, null)
        };
        // The outgoing graph is destroyed here rather than deferred to shutdown.
        assert_eq!(unsafe { hirari_engine_track_count(table.get()) }, 0);
        assert_eq!(destroyed_tracks(), vec![2]);
        assert_eq!(live_handles(), 0);
        assert!(!unsafe { crate::bus_audio::hirari_bus_system_is_present(buses, 5) });
        unsafe { crate::bus_audio::hirari_bus_system_destroy(buses) };
    }

    #[test]
    fn structural_sync_reconciles_buses_publishes_then_compensates() {
        let _guard = serialize();
        let table = Table::new();
        set_facts(1, 0, NO_BUS_ID, 0);
        set_facts(2, crate::track_edit_host::TRACK_TYPE_BUS, 5, 256);
        table.push(1);
        table.push(2);
        let buses = crate::bus_audio::hirari_bus_system_create();
        let pdc = crate::native_pdc::hirari_native_pdc_create();
        assert!(unsafe { crate::native_pdc::hirari_native_pdc_bind_control_thread(pdc) });
        unsafe { hirari_engine_sync_structure(table.get(), buses, std::ptr::null_mut(), pdc, 9) };
        // The walk publishes the graph the callback traverses.
        assert_eq!(table.snapshot().audio_config_generation, 9);
        // Only the bus actually in the graph is presented to the audio path.
        assert!(unsafe { crate::bus_audio::hirari_bus_system_is_present(buses, 5) });
        assert!(!unsafe { crate::bus_audio::hirari_bus_system_is_present(buses, 0) });
        // The bus carries the latency, so the audio track is the one compensated.
        assert_eq!(pdc_offsets(), vec![(1, 256), (2, 0)]);
        unsafe { crate::native_pdc::hirari_native_pdc_destroy(pdc) };
        unsafe { crate::bus_audio::hirari_bus_system_destroy(buses) };
    }

    #[test]
    fn null_handles_answer_defaults() {
        let _guard = serialize();
        let table = Table::new();
        table.push(1);
        assert!(unsafe { hirari_engine_track_table_create(std::ptr::null()) }.is_null());
        assert_eq!(unsafe { hirari_engine_track_count(std::ptr::null()) }, 0);
        assert!(unsafe { hirari_engine_track_ptrs(std::ptr::null()) }.is_null());
        assert!(unsafe { hirari_engine_track_find(std::ptr::null(), 1) }.is_null());
        assert_eq!(
            unsafe { hirari_engine_track_index_of(std::ptr::null(), 1) },
            usize::MAX
        );
        assert!(unsafe { hirari_engine_track_retain(std::ptr::null(), 1) }.is_null());
        let rejected = [new_handle(2), new_handle(3)];
        assert!(!unsafe {
            hirari_engine_track_insert(std::ptr::null_mut(), usize::MAX, rejected[0], raw(2))
        });
        assert!(!unsafe {
            hirari_engine_track_insert(std::ptr::null_mut(), 0, rejected[1], raw(3))
        });
        for owner in rejected {
            release_handle(owner);
        }
        assert!(unsafe { hirari_engine_track_detach(std::ptr::null_mut(), 1) }.is_null());
        unsafe {
            hirari_engine_track_defer_all(std::ptr::null_mut());
            hirari_engine_track_publish(std::ptr::null_mut(), 1, true);
            hirari_engine_track_snapshot(std::ptr::null(), std::ptr::null_mut());
            hirari_engine_track_table_destroy(std::ptr::null_mut());
            hirari_engine_sync_structure(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                1,
            );
            hirari_engine_reset_project(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        };
        assert!(pdc_offsets().is_empty());
        let buffers = [raw(1), raw(2)];
        let mut snapshot = HirariTrackSnapshot {
            active_tracks: buffers.as_ptr(),
            track_map: unsafe { buffers.as_ptr().add(1) },
            track_count: 7,
            audio_config_generation: 7,
        };
        unsafe { hirari_engine_track_snapshot(std::ptr::null(), &mut snapshot) };
        assert_eq!(snapshot, HirariTrackSnapshot::default());
        // The live graph is untouched by every null-handle call above.
        assert_eq!(unsafe { hirari_engine_track_count(table.get()) }, 1);
    }

    #[test]
    fn dropping_the_table_releases_every_handle() {
        let _guard = serialize();
        let table = Table::new();
        table.push(71);
        table.push(72);
        let detached = unsafe { hirari_engine_track_detach(table.get(), 72) };
        release_handle(detached);
        table.publish(1, true);
        assert_eq!(live_handles(), 2);
        drop(table);
        assert_eq!(destroyed_tracks(), vec![71, 72]);
        assert_eq!(live_handles(), 0);
    }
}
