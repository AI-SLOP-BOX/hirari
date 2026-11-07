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
mod tests {
    use super::*;
    use crate::engine_track::{
        hirari_track_delay_samples, hirari_track_id, hirari_track_new, hirari_track_owner_object,
        hirari_track_owner_release, hirari_track_pdc_requested, hirari_track_resolve_bus,
        hirari_track_set_delay_samples, TRACK_TYPE_AUDIO,
    };
    use std::sync::Weak;

    /// Builds a track and hands the reference it owns to the graph, together
    /// with the object pointer every entry point addresses it by and a
    /// non-owning witness for the track's lifetime.
    struct Made {
        owner: *mut c_void,
        object: *mut c_void,
        witness: Weak<Track>,
    }

    fn build(id: u32, kind: u32) -> Made {
        let name = b"t";
        let mut object = std::ptr::null_mut();
        let owner = unsafe {
            hirari_track_new(
                id,
                name.as_ptr(),
                name.len(),
                kind,
                std::ptr::null_mut(),
                &mut object,
            )
        };
        Made {
            witness: unsafe { Arc::downgrade(&*owner.cast::<Arc<Track>>()) },
            owner,
            object,
        }
    }

    struct Table(*mut c_void);

    impl Table {
        fn new() -> Self {
            Table(unsafe { hirari_engine_track_table_create() })
        }
        fn get(&self) -> *mut c_void {
            self.0
        }
        /// Appends one track and returns it, so a test keeps only the pointers
        /// that stay valid while the graph holds the reference.
        fn add(&self, id: u32, kind: u32) -> (*mut c_void, Weak<Track>) {
            let made = self.commit(id, kind);
            (made.object, made.witness)
        }
        fn commit(&self, id: u32, kind: u32) -> Made {
            let made = build(id, kind);
            assert!(unsafe { hirari_engine_track_insert(self.get(), usize::MAX, made.owner) });
            made
        }
        fn publish(&self, generation: u64, stamp: bool) {
            unsafe { hirari_engine_track_publish(self.get(), generation, stamp) };
        }
        fn locked(&self) -> Vec<*mut c_void> {
            let count = unsafe { hirari_engine_track_count(self.get()) };
            unsafe { std::slice::from_raw_parts(hirari_engine_track_ptrs(self.get()), count) }
                .to_vec()
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

    fn ids(tracks: &[*mut c_void]) -> Vec<u32> {
        tracks
            .iter()
            .map(|&track| unsafe { hirari_track_id(track) })
            .collect()
    }

    fn published_ids(snapshot: &HirariTrackSnapshot) -> Vec<u32> {
        ids(&unsafe {
            std::slice::from_raw_parts(snapshot.active_tracks, snapshot.track_count as usize)
        })
    }

    fn mapped(snapshot: &HirariTrackSnapshot, id: u32) -> bool {
        !unsafe { *snapshot.track_map.add(id as usize) }.is_null()
    }

    fn retained(witness: &Weak<Track>) -> bool {
        witness.upgrade().is_some()
    }

    #[test]
    fn graph_keeps_push_order_and_resolves_ids() {
        let table = Table::new();
        let (first, _) = table.add(7, TRACK_TYPE_AUDIO);
        let (second, _) = table.add(3, TRACK_TYPE_AUDIO);
        assert_eq!(unsafe { hirari_engine_track_count(table.get()) }, 2);
        assert_eq!(table.locked(), vec![first, second]);
        assert_eq!(unsafe { hirari_engine_track_find(table.get(), 3) }, second);
        assert_eq!(unsafe { hirari_engine_track_index_of(table.get(), 3) }, 1);
        assert!(unsafe { hirari_engine_track_find(table.get(), 9) }.is_null());
        assert_eq!(
            unsafe { hirari_engine_track_index_of(table.get(), 9) },
            usize::MAX
        );
    }

    #[test]
    fn insert_clamps_the_index_and_rejects_a_null_owner() {
        let table = Table::new();
        table.add(1, TRACK_TYPE_AUDIO);
        table.add(2, TRACK_TYPE_AUDIO);
        assert!(!unsafe {
            hirari_engine_track_insert(table.get(), usize::MAX, std::ptr::null_mut())
        });
        let tail = build(3, TRACK_TYPE_AUDIO);
        assert!(unsafe { hirari_engine_track_insert(table.get(), 99, tail.owner) });
        let head = build(4, TRACK_TYPE_AUDIO);
        assert!(unsafe { hirari_engine_track_insert(table.get(), 0, head.owner) });
        assert_eq!(ids(&table.locked()), vec![4, 1, 2, 3]);
        assert_eq!(unsafe { hirari_engine_track_index_of(table.get(), 3) }, 3);
    }

    #[test]
    fn detach_returns_a_second_reference_and_defers_the_track() {
        let table = Table::new();
        table.add(11, TRACK_TYPE_AUDIO);
        let kept = table.commit(12, TRACK_TYPE_AUDIO);
        let detached = unsafe { hirari_engine_track_detach(table.get(), 11) };
        assert!(!detached.is_null());
        assert_eq!(unsafe { hirari_track_id(hirari_track_owner_object(detached)) }, 11);
        assert_eq!(unsafe { hirari_engine_track_count(table.get()) }, 1);
        // The caller's reference is gone but the deferred list still holds the
        // track, so no audio-thread pointer can dangle.
        unsafe { hirari_track_owner_release(detached) };
        assert!(unsafe { hirari_engine_track_detach(table.get(), 99) }.is_null());
        // Dropping the table releases every track the callback may have seen.
        assert!(retained(&kept.witness));
        drop(table);
        assert!(!retained(&kept.witness));
    }

    #[test]
    fn publish_stamps_the_generation_and_detach_clears_the_id_map() {
        let table = Table::new();
        table.add(21, TRACK_TYPE_AUDIO);
        table.add(22, TRACK_TYPE_AUDIO);
        table.publish(7, true);
        let snapshot = table.snapshot();
        assert_eq!(published_ids(&snapshot), vec![21, 22]);
        assert_eq!(snapshot.audio_config_generation, 7);
        assert!(mapped(&snapshot, 21) && mapped(&snapshot, 22));
        unsafe { hirari_track_owner_release(hirari_engine_track_detach(table.get(), 21)) };
        table.publish(0, false);
        let snapshot = table.snapshot();
        assert_eq!(published_ids(&snapshot), vec![22]);
        assert_eq!(snapshot.audio_config_generation, 7);
        assert!(!mapped(&snapshot, 21));
    }

    #[test]
    fn publish_without_a_stamp_keeps_the_previous_generation() {
        let table = Table::new();
        table.add(1, TRACK_TYPE_AUDIO);
        table.publish(3, true);
        assert_eq!(table.snapshot().audio_config_generation, 3);
        table.add(2, TRACK_TYPE_AUDIO);
        table.publish(99, false);
        assert_eq!(table.snapshot().audio_config_generation, 3);
        assert_eq!(table.snapshot().track_count, 2);
        table.publish(4, true);
        assert_eq!(table.snapshot().audio_config_generation, 4);
    }

    #[test]
    fn publication_writes_the_buffer_the_callback_is_not_reading() {
        let table = Table::new();
        table.add(31, TRACK_TYPE_AUDIO);
        table.add(32, TRACK_TYPE_AUDIO);
        table.publish(1, true);
        let held = table.snapshot();
        assert_eq!(published_ids(&held), vec![31, 32]);
        unsafe {
            hirari_track_owner_release(hirari_engine_track_detach(table.get(), 31));
            hirari_track_owner_release(hirari_engine_track_detach(table.get(), 32));
        }
        table.publish(2, true);
        assert_eq!(published_ids(&table.snapshot()), Vec::<u32>::new());
        // The realtime callback may still be traversing the buffer it read.
        assert_eq!(published_ids(&held), vec![31, 32]);
    }

    #[test]
    fn publication_clamps_to_the_realtime_capacity() {
        let table = Table::new();
        for id in 1..=(MAX_ACTIVE_TRACKS as u32 + 2) {
            table.add(id, TRACK_TYPE_AUDIO);
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
        let table = Table::new();
        let id = TRACK_MAP_LEN as u32 + 5;
        table.add(id, TRACK_TYPE_AUDIO);
        table.add(9, TRACK_TYPE_AUDIO);
        table.publish(1, true);
        let snapshot = table.snapshot();
        assert_eq!(published_ids(&snapshot), vec![id, 9]);
        assert!(mapped(&snapshot, 9));
    }

    #[test]
    fn retain_answers_a_second_reference_for_the_graph() {
        let table = Table::new();
        table.add(41, TRACK_TYPE_AUDIO);
        let kept = table.commit(42, TRACK_TYPE_AUDIO);
        let one = unsafe { hirari_engine_track_retain(table.get(), 42) };
        assert_eq!(unsafe { hirari_track_owner_object(one) }, kept.object);
        unsafe { hirari_track_owner_release(one) };
        assert!(unsafe { hirari_engine_track_retain(table.get(), 99) }.is_null());
        assert!(retained(&kept.witness));
    }

    #[test]
    fn defer_all_holds_the_graph_until_the_table_dies() {
        let table = Table::new();
        let first = table.commit(51, TRACK_TYPE_AUDIO);
        let second = table.commit(52, TRACK_TYPE_AUDIO);
        unsafe { hirari_engine_track_defer_all(table.get()) };
        assert_eq!(unsafe { hirari_engine_track_count(table.get()) }, 0);
        assert!(table.locked().is_empty());
        assert!(retained(&first.witness) && retained(&second.witness));
        drop(table);
        assert!(!retained(&first.witness) && !retained(&second.witness));
    }

    #[test]
    fn reset_project_releases_the_graph_and_resets_the_managers() {
        let table = Table::new();
        let bus = table.commit(2, TRACK_TYPE_BUS);
        unsafe { hirari_track_resolve_bus(bus.object, std::ptr::null_mut(), true) };
        let buses = crate::bus_audio::hirari_bus_system_create();
        let null = std::ptr::null_mut();
        unsafe { hirari_engine_sync_structure(table.get(), buses, null, null, 1) };
        assert!(unsafe { crate::bus_audio::hirari_bus_system_is_present(buses, 2) });
        unsafe {
            hirari_engine_reset_project(table.get(), null, null, null, null, buses, null, null)
        };
        // The outgoing graph is released here rather than deferred to shutdown.
        assert_eq!(unsafe { hirari_engine_track_count(table.get()) }, 0);
        assert!(!retained(&bus.witness));
        assert!(!unsafe { crate::bus_audio::hirari_bus_system_is_present(buses, 2) });
        unsafe { crate::bus_audio::hirari_bus_system_destroy(buses) };
    }

    #[test]
    fn structural_sync_reconciles_buses_publishes_then_compensates() {
        let table = Table::new();
        let audio = table.commit(1, TRACK_TYPE_AUDIO);
        let bus = table.commit(5, TRACK_TYPE_BUS);
        unsafe { hirari_track_resolve_bus(bus.object, std::ptr::null_mut(), true) };
        // The user delay is the part of the delay line a graph rebuild must
        // never erase, so seed it before the solve runs.
        unsafe { hirari_track_set_delay_samples(audio.object, 64) };
        let buses = crate::bus_audio::hirari_bus_system_create();
        let pdc = crate::native_pdc::hirari_native_pdc_create();
        assert!(unsafe { crate::native_pdc::hirari_native_pdc_bind_control_thread(pdc) });
        unsafe { hirari_engine_sync_structure(table.get(), buses, std::ptr::null_mut(), pdc, 9) };
        // The walk publishes the graph the callback traverses.
        assert_eq!(table.snapshot().audio_config_generation, 9);
        // Only the bus actually in the graph is presented to the audio path.
        assert!(unsafe { crate::bus_audio::hirari_bus_system_is_present(buses, 5) });
        assert!(!unsafe { crate::bus_audio::hirari_bus_system_is_present(buses, 1) });
        // A chain without inserts reports no latency, so the solver owes every
        // track a zero offset — and the manual delay still reads back.
        assert_eq!(unsafe { hirari_track_pdc_requested(audio.object) }, 0);
        assert_eq!(unsafe { hirari_track_pdc_requested(bus.object) }, 0);
        assert_eq!(unsafe { hirari_track_delay_samples(audio.object) }, 64);
        unsafe { crate::native_pdc::hirari_native_pdc_destroy(pdc) };
        unsafe { crate::bus_audio::hirari_bus_system_destroy(buses) };
    }

    #[test]
    fn null_handles_answer_defaults() {
        let table = Table::new();
        table.add(1, TRACK_TYPE_AUDIO);
        assert_eq!(unsafe { hirari_engine_track_count(std::ptr::null()) }, 0);
        assert!(unsafe { hirari_engine_track_ptrs(std::ptr::null()) }.is_null());
        assert!(unsafe { hirari_engine_track_find(std::ptr::null(), 1) }.is_null());
        assert_eq!(
            unsafe { hirari_engine_track_index_of(std::ptr::null(), 1) },
            usize::MAX
        );
        assert!(unsafe { hirari_engine_track_retain(std::ptr::null(), 1) }.is_null());
        let first = build(2, TRACK_TYPE_AUDIO);
        let second = build(3, TRACK_TYPE_AUDIO);
        assert!(!unsafe {
            hirari_engine_track_insert(std::ptr::null_mut(), usize::MAX, first.owner)
        });
        assert!(!unsafe { hirari_engine_track_insert(std::ptr::null_mut(), 0, second.owner) });
        for owner in [first.owner, second.owner] {
            unsafe { hirari_track_owner_release(owner) };
        }
        assert!(unsafe { hirari_engine_track_detach(std::ptr::null_mut(), 1) }.is_null());
        unsafe { hirari_engine_track_table_destroy(std::ptr::null_mut()) };
        unsafe {
            hirari_engine_track_defer_all(std::ptr::null_mut());
            hirari_engine_track_publish(std::ptr::null_mut(), 1, true);
            hirari_engine_track_snapshot(std::ptr::null(), std::ptr::null_mut());
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
        let buffers = [table.locked()[0], std::ptr::null_mut()];
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
        let table = Table::new();
        let first = table.commit(71, TRACK_TYPE_AUDIO);
        let second = table.commit(72, TRACK_TYPE_AUDIO);
        let detached = unsafe { hirari_engine_track_detach(table.get(), 72) };
        unsafe { hirari_track_owner_release(detached) };
        table.publish(1, true);
        assert!(retained(&first.witness) && retained(&second.witness));
        drop(table);
        assert!(!retained(&first.witness) && !retained(&second.witness));
    }

    #[test]
    fn a_retained_handle_keeps_the_object_readable_after_the_graph_goes() {
        let table = Table::new();
        table.add(61, TRACK_TYPE_AUDIO);
        let retained = unsafe { hirari_engine_track_retain(table.get(), 61) };
        let object = unsafe { hirari_track_owner_object(retained) };
        drop(table);
        // A control walk that took the track out of the graph still reads a
        // consistent identity from it, because its own reference holds.
        assert_eq!(unsafe { hirari_track_id(object) }, 61);
        unsafe { hirari_track_owner_release(retained) };
    }
}
