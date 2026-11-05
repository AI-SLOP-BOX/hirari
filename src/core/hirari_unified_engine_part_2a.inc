// The preview-sampler swap and its quiescence protocol are Rust-side now
// (engine_transport.rs): the delegate asks the block store whether the
// callback is running, joins it on the control thread and drives the sampler
// clear/reclaim/set externs directly, so no engine method forwards either.
// The block-size, latency and callback-join answers come straight from that
// store too: the device format, the master fader and the in-flight count are
// its state, so this facade keeps no reader for any of them.
void HirariUnifiedEngine::set_playing(bool p) {
    // The voice release and the playback panic are part of stopping; Rust's
    // set_playing asks the callback to perform them on its next block.
    hirari_engine_block_set_playing(handle(kHirariHandleEngineBlock), p ? 1u : 0u);
    if (!p) {
        // The callback is quiescent once transport is stopped. Reclaim old
        // immutable region generations on the control thread only.
        hirari_engine_block_join_callbacks(handle(kHirariHandleEngineBlock));
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (void* track : lockedTracks()) {
            hirari_region_store_trim_retired(track);
        }
    }
}

void HirariUnifiedEngine::new_project() {
    set_playing(false);
    hirari_engine_block_join_callbacks(handle(kHirariHandleEngineBlock));
    // Input monitoring is a live device assignment, not project data. Clear
    // it before reusing any track IDs in the new project.
    hirari_input_blocks_reset(handle(kHirariHandleInputBlocks));
    hirari_engine_command_ring_clear(handle(kHirariHandleEngineRings));
    hirari_region_jobs_clear(this);
    hirari_engine_block_set_test_tone(handle(kHirariHandleEngineBlock), 0);
    hirari_engine_block_set_playhead(handle(kHirariHandleEngineBlock), 0);
    hirari_mpe_state_reset(handle(kHirariHandleMpeState));
    hirari_mpe_manager_reset(handle(kHirariHandleMpeManager));
    hirari_undo_manager_clear(handle(kHirariHandleUndo));
    hirari_native_pdc_reset_for_project(handle(kHirariHandlePdc));
    hirari_sidechain_engine_reset(handle(kHirariHandleSidechainEngine));
    hirari_routing_engine_reset(handle(kHirariHandleRouting));
    hirari_bus_system_reset(handle(kHirariHandleBusSystem));
    hirari_control_room_reset(handle(kHirariHandleControlRoom));
    hirari_tempo_map_set_current_bpm(handle(kHirariHandleTempo), 120.0f);
    hirari_engine_block_set_master_gain(handle(kHirariHandleEngineBlock), 1.0f);
    // Project tonality (root + scale) is no longer reset here: that state
    // moved into Core, and HirariCore::new_project() resets the same
    // (root 0, Major) defaults alongside the other project-scoped mirrors.
    // Region ids restart with the project, so cached peaks must not survive.
    hirari_region_waveform_clear(this);
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    hirari_sidechain_routes_clear(handle(kHirariHandleSidechainRoutes));
    // Keep old Track objects alive while a CoreAudio callback may still hold
    // the previous published pointers; the table destroys them at shutdown.
    hirari_engine_track_defer_all(handle(kHirariHandleTrackTable));
    mark_project_layout_changed();
    hirari_ids_reset_project(this);
    // Start with one audio track so a new project is immediately usable for
    // importing or recording, instead of presenting a dead empty canvas.
    void* bootstrap = nullptr;
    void* bootstrapOwner = hirari_track_new(
        1, reinterpret_cast<const uint8_t*>("Audio 1"), 7, kHirariTrackKindAudio,
        hirari_engine_block_layout_revision(handle(kHirariHandleEngineBlock)), &bootstrap);
    if (bootstrapOwner) {
        (void)hirari_engine_block_prepare_track(handle(kHirariHandleEngineBlock), bootstrap);
        (void)hirari_engine_track_insert(handle(kHirariHandleTrackTable), 0, bootstrapOwner);
        mark_project_layout_changed();
    }
    publishTrackSnapshotLocked(true);
}

