//! Host-ops wiring for the bounce driver and the region edit host,
//! split out of engine_native.rs.
use super::*;

pub(crate) fn bounce_ops() -> &'static crate::bounce_driver::HirariBounceHostOps {
    static OPS: OnceLock<crate::bounce_driver::HirariBounceHostOps> = OnceLock::new();
    OPS.get_or_init(|| crate::bounce_driver::HirariBounceHostOps {
        run: Some(bounce_run),
        reset: Some(bounce_reset),
        store: Some(bounce_store),
        render: Some(bounce_render),
        render_chunk: Some(bounce_render_chunk),
        read_transport: Some(bounce_read_transport),
        set_transport: Some(bounce_set_transport),
    })
}

unsafe extern "C" fn bounce_run(host: *mut c_void, path: *const u8, length: usize, format: u32) -> u8 {
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let bytes = unsafe { std::slice::from_raw_parts(path, length) };
    let Ok(path) = std::str::from_utf8(bytes) else { return 0 };
    u8::from(engine.bounce_project(path, format))
}

/// Clears stateful processor history so a bounce is reproducible.
unsafe extern "C" fn bounce_reset(host: *mut c_void) {
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let limiter = engine.handle(H_MASTER_LIMITER);
    if !limiter.is_null() {
        unsafe { crate::master_limiter::hirari_master_limiter_reset(limiter) };
    }
    let _lock = engine.lock_tracks();
    for track in engine.published_tracks_live_locked() {
        if !track.is_null() {
            unsafe { crate::engine_track::hirari_track_reset_for_offline_render(track) };
        }
    }
}

/// The plan walk is Rust's (bounce_driver.rs): this only names the stores.
unsafe extern "C" fn bounce_store(host: *mut c_void, kind: u32) -> *mut c_void {
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    match kind {
        0 => engine.track_table(),                       // kHirariBounceStoreTrackTable
        1 => engine.block(),                             // kHirariBounceStoreEngineBlock
        2 => engine.handle(H_MIDI_SNAPSHOT_STORE),       // kHirariBounceStoreMidiSnapshot
        _ => engine.handle(H_MASTER_LIMITER),            // kHirariBounceStoreMasterLimiter
    }
}

/// The driver's channel pairs are wrapped, not copied, so the offline graph
/// renders straight into the caller's block buffers.
unsafe extern "C" fn bounce_render(host: *mut c_void, left: *mut f32, right: *mut f32, frames: u32) {
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let mut channels: [*mut f32; 2] = [left, right];
    let view = { crate::audio_buffer_ops::hirari_audio_buffer_storage_create() };
    unsafe {
        let _ = crate::audio_buffer_ops::hirari_audio_buffer_storage_wrap_channels(
            view,
            channels.as_mut_ptr(),
            2,
            frames,
        );
        engine.process_block(view, 0, frames);
        crate::audio_buffer_ops::hirari_audio_buffer_storage_destroy(view);
    }
}

/// Tracks size their buffers to the engine block size and silently skip larger
/// blocks; never render past the prepared block.
unsafe extern "C" fn bounce_render_chunk(host: *mut c_void) -> u32 {
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let block = unsafe {
        crate::engine_block_host::hirari_engine_block_device_block_frames(engine.block())
    };
    if block == 0 {
        1024
    } else {
        block.min(1024)
    }
}

unsafe extern "C" fn bounce_read_transport(host: *mut c_void, playhead: *mut u64, playing: *mut u8) {
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    unsafe {
        *playhead = crate::engine_block_host::hirari_engine_block_playhead(engine.block());
        *playing = crate::engine_block_host::hirari_engine_block_is_playing(engine.block());
    }
}

unsafe extern "C" fn bounce_set_transport(host: *mut c_void, playhead: u64, playing: u8) {
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    unsafe {
        crate::engine_block_host::hirari_engine_block_set_playhead(engine.block(), playhead);
        crate::engine_block_host::hirari_engine_block_set_playing(engine.block(), playing);
    }
}

// ---------------------------------------------------------------------------
// Region edit host callbacks.
//
// The Rust region store owns the table, its undo capture and its snapshot
// publication; what remains is only the engine surface it cannot own: the
// track registry, the handle lifetimes for imported audio and the
// pitch-analysis gate on the Track itself.
// ---------------------------------------------------------------------------

unsafe extern "C" fn region_host_list_tracks(host: *mut c_void, out: *mut u32, capacity: usize) -> usize {
    if host.is_null() {
        return 0;
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let _lock = engine.lock_tracks();
    let mut count = 0usize;
    for track in engine.published_tracks_live_locked() {
        if track.is_null() {
            continue;
        }
        if !out.is_null() && count < capacity {
            unsafe { *out.add(count) = crate::engine_track::hirari_track_id(track) };
        }
        count += 1;
    }
    count
}

unsafe extern "C" fn region_host_tempo_state(host: *mut c_void) -> *mut c_void {
    if host.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*(host as *const HirariUnifiedEngine)).handle(H_TEMPO) }
}

unsafe extern "C" fn region_host_undo_state(host: *mut c_void) -> *mut c_void {
    if host.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*(host as *const HirariUnifiedEngine)).handle(H_UNDO) }
}

unsafe extern "C" fn region_host_sample_rate(host: *mut c_void) -> f64 {
    if host.is_null() {
        return 0.0;
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    unsafe { crate::engine_block_host::hirari_engine_block_device_sample_rate(engine.block()) }
}

unsafe extern "C" fn region_host_track_identity(host: *mut c_void, track_id: u32) -> usize {
    if host.is_null() {
        return 0;
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let _lock = engine.lock_tracks();
    engine.find_track_locked(track_id) as usize
}

unsafe extern "C" fn region_host_retain_track(host: *mut c_void, track_id: u32) -> *mut c_void {
    if host.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*(host as *const HirariUnifiedEngine)).take_track_owner(track_id) }
}

unsafe extern "C" fn region_host_release_track(handle: *mut c_void) {
    unsafe { crate::engine_track::hirari_track_owner_release(handle) };
}

unsafe extern "C" fn region_host_pitch_begin(track_owner: *mut c_void) -> bool {
    unsafe { crate::engine_track::hirari_track_pitch_begin(owner_object(track_owner)) }
}

unsafe extern "C" fn region_host_pitch_finish(track_owner: *mut c_void) {
    unsafe { crate::engine_track::hirari_track_pitch_finish(owner_object(track_owner)) };
}

unsafe extern "C" fn region_host_note_snapshot(track_owner: *mut c_void, region_id: u32) -> *mut c_void {
    unsafe {
        crate::region_store::hirari_region_store_note_snapshot(
            owner_object(track_owner),
            region_id,
        )
    }
}

/// The live track behind an owner handle, or null.
unsafe fn owner_object(owner: *mut c_void) -> *mut c_void {
    unsafe { crate::engine_track::hirari_track_owner_object(owner) }
}

pub(crate) fn region_edit_host_ops() -> crate::region_edit_host::HirariRegionHostOps {
    crate::region_edit_host::HirariRegionHostOps {
        // The Rust region store answers the region-data callbacks.
        audio_view: Some(crate::storage_handle::hirari_storage_handle_view),
        release_audio: Some(crate::storage_handle::hirari_storage_handle_release),
        list_tracks: Some(region_host_list_tracks),
        tempo_state: Some(region_host_tempo_state),
        undo_state: Some(region_host_undo_state),
        sample_rate: Some(region_host_sample_rate),
        // region_path and insert_region: Rust-owned.
        track_identity: Some(region_host_track_identity),
        retain_track: Some(region_host_retain_track),
        release_track: Some(region_host_release_track),
        pitch_begin: Some(region_host_pitch_begin),
        pitch_finish: Some(region_host_pitch_finish),
        note_snapshot: Some(region_host_note_snapshot),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// The other edit-host tables. Every store they name is a session-bag slot, so
// the handlers are one-line resolvers plus the two engine operations that must
// stay here: the layout mark and the graph resync.
// ---------------------------------------------------------------------------

unsafe extern "C" fn track_edit_store(host: *mut c_void, kind: u32) -> *mut c_void {
    if host.is_null() {
        return ptr::null_mut();
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    match kind {
        0 => engine.track_table(),                  // kHirariTrackEditStoreTrackTable
        1 => engine.block(),                        // kHirariTrackEditStoreEngineBlock
        2 => engine.handle(H_SIDECHAIN_ENGINE),     // kHirariTrackEditStoreSidechain
        3 => engine.handle(H_SIDECHAIN_ROUTES),     // kHirariTrackEditStoreSidechainRoutes
        4 => engine.handle(H_ROUTING),              // kHirariTrackEditStoreRouting
        5 => unsafe {
            crate::engine_block_host::hirari_engine_block_layout_revision(engine.block())
        },                                          // kHirariTrackEditStoreLayoutRevision
        6 => engine.handle(H_BUS_SYSTEM),           // kHirariTrackEditStoreBusSystem
        _ => engine.handle(H_ROUTING),
    }
}

unsafe extern "C" fn track_edit_undo_state(host: *mut c_void) -> *mut c_void {
    region_host_undo_state(host)
}

unsafe extern "C" fn track_edit_sync(host: *mut c_void) {
    if !host.is_null() {
        unsafe { (*(host as *const HirariUnifiedEngine)).sync_structural_changes() };
    }
}

unsafe extern "C" fn routing_mark_layout_changed(host: *mut c_void) {
    if !host.is_null() {
        unsafe { (*(host as *const HirariUnifiedEngine)).mark_project_layout_changed() };
    }
}

unsafe extern "C" fn routing_sync_structural(host: *mut c_void) {
    // sync_structural_changes takes the graph lock itself; Rust calls this only
    // after every mutation callback has released its lock.
    track_edit_sync(host);
}

unsafe extern "C" fn routing_store_routing(host: *mut c_void) -> *mut c_void {
    routing_store_of(host, H_ROUTING)
}

unsafe extern "C" fn routing_store_routes(host: *mut c_void) -> *mut c_void {
    routing_store_of(host, H_SIDECHAIN_ROUTES)
}

unsafe extern "C" fn routing_store_engine(host: *mut c_void) -> *mut c_void {
    routing_store_of(host, H_SIDECHAIN_ENGINE)
}

unsafe extern "C" fn routing_block_store(host: *mut c_void) -> *mut c_void {
    routing_store_of(host, H_ENGINE_BLOCK)
}

unsafe extern "C" fn routing_store_of(host: *mut c_void, kind: u32) -> *mut c_void {
    if host.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*(host as *const HirariUnifiedEngine)).handle(kind) }
}

unsafe extern "C" fn sandbox_maintenance_statuses(
    host: *mut c_void,
    context: *mut c_void,
    visit: Option<crate::plugin_sandbox_maintenance::StatusVisitorFn>,
) -> bool {
    if host.is_null() {
        return false;
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let _lock = engine.lock_tracks();
    for track in engine.published_tracks_live_locked() {
        unsafe {
            crate::engine_track::hirari_track_visit_sandbox_statuses(track, context, visit);
        }
    }
    true
}

/// The restart can block on a worker process, so the track is held by handle
/// and the topology lock is already released around it.
unsafe extern "C" fn sandbox_maintenance_maintain(
    host: *mut c_void,
    track_id: u32,
    auto_restart: bool,
    context: *mut c_void,
    visit: Option<crate::plugin_sandbox_maintenance::StatusVisitorFn>,
) -> u32 {
    if host.is_null() {
        return 0;
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let owner = engine.take_track_owner(track_id);
    if owner.is_null() {
        return 0;
    }
    let recovered = unsafe {
        crate::engine_track::hirari_track_maintain_sandboxes(
            owner_object(owner),
            auto_restart,
            context,
            visit,
        )
    };
    unsafe { crate::engine_track::hirari_track_owner_release(owner) };
    recovered
}

unsafe extern "C" fn sandbox_maintenance_publish_event(
    host: *mut c_void,
    event_type: u32,
    track_id: u32,
    value: f32,
    label: *const u8,
    label_length: usize,
) {
    if host.is_null() {
        return;
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let block = engine.block();
    unsafe {
        crate::engine_rings::hirari_engine_event_ring_push(
            engine.handle(H_ENGINE_RINGS).cast(),
            crate::engine_block_host::hirari_engine_block_playhead(block),
            event_type,
            track_id,
            value,
            label,
            label_length,
        );
    }
}

unsafe extern "C" fn expression_map_track_articulation(host: *mut c_void, track_id: u32) -> *mut c_void {
    if host.is_null() {
        return ptr::null_mut();
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let track = engine.find_published_track(track_id);
    if track.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::engine_track::hirari_track_storage(track, 5) } // STORAGE_ARTICULATION
}

unsafe extern "C" fn expression_map_set_map_json(
    host: *mut c_void,
    track_id: u32,
    kind: u8,
    data: *const u8,
    length: usize,
) -> bool {
    if host.is_null() || (length != 0 && data.is_null()) {
        return false;
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let track = engine.find_published_track(track_id);
    unsafe { crate::engine_track::hirari_track_expression_map_set(track, kind, data, length) }
}

unsafe extern "C" fn freeze_acquire(host: *mut c_void, tid: u32, wait: bool) -> *mut c_void {
    if host.is_null() {
        return ptr::null_mut();
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    if wait {
        engine.join_audio_callbacks();
    }
    engine.take_track_owner(tid)
}

unsafe extern "C" fn freeze_block_size(host: *mut c_void) -> u32 {
    if host.is_null() {
        return 0;
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    unsafe {
        crate::engine_block_host::hirari_engine_block_device_block_frames(engine.block())
    }
}

/// Every host table is installed once, before any caller can reach the engine,
/// so the install order is deterministic instead of depending on a linker's
/// cross-file static order.
pub(crate) fn install_native_host_tables() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| unsafe {
        crate::track_edit_host::hirari_track_edit_host_install(
            &crate::track_edit_host::HirariTrackEditHostOps {
                list_tracks: Some(region_host_list_tracks),
                list_regions: Some(crate::region_edit_host::hirari_region_store_list_regions),
                store: Some(track_edit_store),
                // Undo capture lives in the Rust track edit host.
                record_replay: None,
                sync_structural_changes: Some(track_edit_sync),
                undo_state: Some(track_edit_undo_state),
            },
        );
        crate::plugin_sandbox_maintenance::hirari_plugin_sandbox_maintenance_host_install(
            &crate::plugin_sandbox_maintenance::HirariPluginSandboxHostOps {
                statuses: Some(sandbox_maintenance_statuses),
                maintain: Some(sandbox_maintenance_maintain),
                publish_event: Some(sandbox_maintenance_publish_event),
            },
        );
        crate::routing_edit_host::hirari_routing_edit_host_install(
            &crate::routing_edit_host::HirariRoutingHostOps {
                routing_store: Some(routing_store_routing),
                sidechain_routes_store: Some(routing_store_routes),
                sidechain_engine_store: Some(routing_store_engine),
                // Sidechain context and registration live in the Rust routing
                // edit host; undo capture lives there too.
                sidechain_context: None,
                register_sidechain: None,
                mark_layout_changed: Some(routing_mark_layout_changed),
                sync_structural: Some(routing_sync_structural),
                record_undo: None,
                undo_state: Some(track_edit_undo_state),
                engine_block_store: Some(routing_block_store),
            },
        );
        crate::expression_map_edit_host::hirari_expression_map_edit_host_install(
            &crate::expression_map_edit_host::HirariExpressionMapEditHostOps {
                track_articulation: Some(expression_map_track_articulation),
                set_map_json: Some(expression_map_set_map_json),
            },
        );
        crate::track_freeze_host::hirari_track_freeze_host_install(
            &crate::track_freeze_host::HirariTrackFreezeHostOps {
                // The owner handle is the Rust Arc's boxed clone, so it keeps
                // the track alive after the topology lock is released.
                acquire: Some(freeze_acquire),
                release: Some(crate::engine_track::hirari_track_owner_release),
                block_size: Some(freeze_block_size),
            },
        );
        // The tracks and their storages are Rust objects, so the spatial-edit
        // host needs no native table, and neither do the automation and
        // control edits or command admission. The region storage host must
        // install before the region edit host, which routes its data callbacks
        // only once it answers.
        crate::engine_track::hirari_track_region_host_install();
        crate::region_edit_host::hirari_region_host_install(&region_edit_host_ops());
    });
}

// ---------------------------------------------------------------------------
// The session wrapper.
// ---------------------------------------------------------------------------
