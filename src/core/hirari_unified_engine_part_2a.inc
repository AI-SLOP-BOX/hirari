void HirariUnifiedEngine::waitForAudioCallbacks() const noexcept {
    // This is a control/shutdown path, never the real-time callback. Yielding
    // avoids destroying Track objects while a callback still owns raw state
    // pointers, without introducing a mutex into the audio path.
    while (m_audioCallbacksInFlight.load(std::memory_order_acquire) != 0) {
        std::this_thread::yield();
    }
}

void HirariUnifiedEngine::set_preview_sample(const float* samples, size_t count, double sourceRate) {
    if (samples == nullptr || count == 0 || !std::isfinite(sourceRate) || sourceRate <= 0.0) {
        clear_preview_sample();
        return;
    }
    // Retired generations are reclaimed only while the callback is quiescent,
    // so a running preview never has its buffer freed underneath it.
    if (!hirari_engine_block_is_playing(m_engineBlock)) {
        hirari_preview_sampler_clear(m_previewSampler);
        waitForAudioCallbacks();
        hirari_preview_sampler_reclaim(m_previewSampler);
    }
    hirari_preview_sampler_set(m_previewSampler, samples, count, sourceRate);
}

void HirariUnifiedEngine::clear_preview_sample() {
    hirari_preview_sampler_clear(m_previewSampler);
    if (!hirari_engine_block_is_playing(m_engineBlock)) {
        waitForAudioCallbacks();
        hirari_preview_sampler_reclaim(m_previewSampler);
    }
}

void HirariUnifiedEngine::set_playing(bool p) {
    // The voice release and the playback panic are part of stopping; Rust's
    // set_playing asks the callback to perform them on its next block.
    hirari_engine_block_set_playing(m_engineBlock, p ? 1u : 0u);
    if (!p) {
        // The callback is quiescent once transport is stopped. Reclaim old
        // immutable region generations on the control thread only.
        waitForAudioCallbacks();
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (Track* track : lockedTracks()) {
            hirari_region_store_trim_retired(track);
        }
    }
}
bool HirariUnifiedEngine::is_playing() const {
    return hirari_engine_block_is_playing(m_engineBlock) != 0;
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
    return queryLockedTrack(tid, 0.0f, [&](const Track& track) {
        return hirari_samples_to_milliseconds(track.getTotalLatencySamples(), sampleRate);
    });
}

float HirariUnifiedEngine::get_track_pdc_compensation_ms(uint32_t tid) const noexcept {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    return queryLockedTrack(tid, 0.0f, [&](const Track& track) {
        return hirari_samples_to_milliseconds(track.getPdcCompensationSamples(), sampleRate);
    });
}

void HirariUnifiedEngine::set_playhead(uint64_t p) { hirari_engine_block_set_playhead(m_engineBlock, p); }
uint64_t HirariUnifiedEngine::get_playhead() const { return hirari_engine_block_playhead(m_engineBlock); }
void HirariUnifiedEngine::set_loop(bool enabled) { hirari_engine_block_set_loop(m_engineBlock, enabled); }
bool HirariUnifiedEngine::set_cycle_range(uint64_t start_sample, uint64_t end_sample, bool enabled) {
    return hirari_engine_block_set_cycle(m_engineBlock, start_sample, end_sample, enabled) != 0;
}
bool HirariUnifiedEngine::is_loop_enabled() const noexcept {
    return hirari_engine_block_loop_enabled(m_engineBlock) != 0;
}
uint64_t HirariUnifiedEngine::cycle_start() const noexcept { return hirari_engine_block_cycle_start(m_engineBlock); }
uint64_t HirariUnifiedEngine::cycle_end() const noexcept { return hirari_engine_block_cycle_end(m_engineBlock); }
void HirariUnifiedEngine::set_test_tone(bool enabled) { hirari_engine_block_set_test_tone(m_engineBlock, enabled); }

bool HirariUnifiedEngine::assign_track_to_vca(uint32_t trackId, uint32_t groupId) {
    if (trackId == 0 || groupId == 0) return false;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        if (!findTrackLocked(trackId)) return false;
    }
    return hirari_vca_assign_track(trackId, groupId);
}


void HirariUnifiedEngine::new_project() {
    set_playing(false);
    waitForAudioCallbacks();
    // Input monitoring is a live device assignment, not project data. Clear
    // it before reusing any track IDs in the new project.
    hirari_input_blocks_reset(m_inputBlocks);
    hirari_engine_command_ring_clear(m_engineRings);
    hirari_region_jobs_clear(this);
    set_test_tone(false);
    set_playhead(0);
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
    auto bootstrapTrack = std::make_shared<Track>(1, "Audio 1", Track::Audio,
                                                  m_projectLayoutRevision);
    bootstrapTrack->prepareToPlay(m_sampleRate.load(std::memory_order_relaxed),
                                  m_blockSize.load(std::memory_order_relaxed));
    Track* bootstrap = bootstrapTrack.get();
    (void)hirari_engine_track_insert(m_trackTable, 0, newTrackHandle(std::move(bootstrapTrack)),
                                       bootstrap);
    mark_project_layout_changed();
    publishTrackSnapshotLocked(true);
}

