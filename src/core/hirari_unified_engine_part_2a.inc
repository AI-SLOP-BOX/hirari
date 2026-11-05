void HirariUnifiedEngine::waitForAudioCallbacks() const noexcept {
    // This is a control/shutdown path, never the real-time callback. Yielding
    // avoids destroying Track objects while a callback still owns raw state
    // pointers, without introducing a mutex into the audio path.
    while (m_audioCallbacksInFlight.load(std::memory_order_acquire) != 0) {
        std::this_thread::yield();
    }
}

// The preview-sampler swap and its quiescence protocol are Rust-side now
// (engine_transport.rs): the delegate asks the block store whether the
// callback is running, joins it on the control thread and drives the sampler
// clear/reclaim/set externs directly, so no engine method forwards either.
void HirariUnifiedEngine::set_playing(bool p) {
    // The voice release and the playback panic are part of stopping; Rust's
    // set_playing asks the callback to perform them on its next block.
    hirari_engine_block_set_playing(m_engineBlock, p ? 1u : 0u);
    if (!p) {
        // The callback is quiescent once transport is stopped. Reclaim old
        // immutable region generations on the control thread only.
        waitForAudioCallbacks();
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (void* track : lockedTracks()) {
            hirari_region_store_trim_retired(track);
        }
    }
}
uint32_t HirariUnifiedEngine::get_block_size() const noexcept {
    return m_blockSize.load(std::memory_order_relaxed);
}
float HirariUnifiedEngine::get_latency_ms() const noexcept {
    uint32_t masterLatency = get_block_size();
    if (m_masterLimiter) {
        masterLatency += hirari_master_limiter_latency(m_masterLimiter);
    }
    return hirari_samples_to_milliseconds(
        masterLatency, m_sampleRate.load(std::memory_order_relaxed));
}

float HirariUnifiedEngine::get_track_latency_ms(uint32_t tid) const noexcept {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    return queryLockedTrack(tid, 0.0f, [&](void* track) {
        return hirari_samples_to_milliseconds(hirari_track_latency_samples(track), sampleRate);
    });
}

float HirariUnifiedEngine::get_track_pdc_compensation_ms(uint32_t tid) const noexcept {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    return queryLockedTrack(tid, 0.0f, [&](void* track) {
        return hirari_samples_to_milliseconds(hirari_track_pdc_requested(track), sampleRate);
    });
}

// The playhead, loop, cycle, test tone and VCA assignment relays are gone:
// engine_transport.rs reads and writes the engine-block store and the global
// VCA manager directly, and the delegate calls it. Only the device-loss and
// new-project paths in this facade still touch the block store, through the
// hirari_engine_block_* externs.

void HirariUnifiedEngine::new_project() {
    set_playing(false);
    waitForAudioCallbacks();
    // Input monitoring is a live device assignment, not project data. Clear
    // it before reusing any track IDs in the new project.
    hirari_input_blocks_reset(m_inputBlocks);
    hirari_engine_command_ring_clear(m_engineRings);
    hirari_region_jobs_clear(this);
    hirari_engine_block_set_test_tone(m_engineBlock, 0);
    hirari_engine_block_set_playhead(m_engineBlock, 0);
    hirari_mpe_state_reset(m_mpeState);
    hirari_mpe_manager_reset(m_mpeManager);
    hirari_undo_manager_clear(m_undoState);
    hirari_native_pdc_reset_for_project(m_pdcState);
    hirari_sidechain_engine_reset(m_sidechainEngine);
    hirari_routing_engine_reset(m_routingEngine);
    hirari_bus_system_reset(m_busSystem);
    hirari_control_room_reset(m_controlRoomState);
    hirari_tempo_map_set_current_bpm(m_tempoState, 120.0f);
    m_masterGain.store(1.0f, std::memory_order_release);
    // Project tonality (root + scale) is no longer reset here: that state
    // moved into Core, and HirariCore::new_project() resets the same
    // (root 0, Major) defaults alongside the other project-scoped mirrors.
    // Region ids restart with the project, so cached peaks must not survive.
    hirari_region_waveform_clear(this);
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    hirari_sidechain_routes_clear(m_sidechainRoutes);
    // Keep old Track objects alive while a CoreAudio callback may still hold
    // the previous published pointers; the table destroys them at shutdown.
    hirari_engine_track_defer_all(m_trackTable);
    mark_project_layout_changed();
    hirari_ids_reset_project(this);
    // Start with one audio track so a new project is immediately usable for
    // importing or recording, instead of presenting a dead empty canvas.
    void* bootstrap = nullptr;
    void* bootstrapOwner = hirari_track_new(
        1, reinterpret_cast<const uint8_t*>("Audio 1"), 7, kHirariTrackKindAudio,
        m_layoutRevision, &bootstrap);
    if (bootstrapOwner) {
        (void)hirari_track_prepare(bootstrap, m_sampleRate.load(std::memory_order_relaxed),
                                   m_blockSize.load(std::memory_order_relaxed));
        (void)hirari_engine_track_insert(m_trackTable, 0, bootstrapOwner);
        mark_project_layout_changed();
    }
    publishTrackSnapshotLocked(true);
}

