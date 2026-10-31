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
    if (!m_isPlaying.load(std::memory_order_acquire)) {
        hirari_preview_sampler_clear(m_previewSampler);
        waitForAudioCallbacks();
        hirari_preview_sampler_reclaim(m_previewSampler);
    }
    hirari_preview_sampler_set(m_previewSampler, samples, count, sourceRate);
}

void HirariUnifiedEngine::trigger_preview_sample() {
    hirari_preview_sampler_trigger(m_previewSampler);
}

void HirariUnifiedEngine::clear_preview_sample() {
    hirari_preview_sampler_clear(m_previewSampler);
    if (!m_isPlaying.load(std::memory_order_acquire)) {
        waitForAudioCallbacks();
        hirari_preview_sampler_reclaim(m_previewSampler);
    }
}

void HirariUnifiedEngine::clear_midi_notes() {
    replace_midi_notes({}, true);
}

bool HirariUnifiedEngine::replace_midi_notes(const std::vector<ScheduledMidiNote>& notes, bool recordUndo) {
    static_assert(sizeof(ScheduledMidiNote) == sizeof(HirariScheduledMidiNote));
    static_assert(offsetof(ScheduledMidiNote, startSample) ==
                  offsetof(HirariScheduledMidiNote, start_sample));
    static_assert(offsetof(ScheduledMidiNote, regionId) ==
                  offsetof(HirariScheduledMidiNote, region_id));
    return hirari_midi_notes_replace(
        m_midiSnapshotStore, reinterpret_cast<const HirariScheduledMidiNote*>(notes.data()),
        notes.size(), recordUndo, undo_state(), log_state());
}

std::vector<HirariUnifiedEngine::ScheduledMidiNote> HirariUnifiedEngine::midi_notes_snapshot() const {
    size_t count = 0;
    auto* notes = hirari_midi_notes_snapshot_take(m_midiSnapshotStore, &count);
    if (!notes) return {};
    std::vector<ScheduledMidiNote> copy(
        reinterpret_cast<const ScheduledMidiNote*>(notes),
        reinterpret_cast<const ScheduledMidiNote*>(notes) + count);
    hirari_midi_notes_snapshot_take_free(notes, count);
    return copy;
}

bool HirariUnifiedEngine::remove_midi_notes_range(uint32_t trackId, uint64_t startSample,
                                                uint64_t endSample) {
    return hirari_midi_notes_edit_range(m_midiSnapshotStore, trackId, startSample, endSample,
                                        0, 0, undo_state(), log_state());
}

bool HirariUnifiedEngine::transpose_midi_notes_range(uint32_t trackId, uint64_t startSample,
                                                   uint64_t endSample, int32_t semitones) {
    return hirari_midi_notes_edit_range(m_midiSnapshotStore, trackId, startSample, endSample,
                                        1, semitones, undo_state(), log_state());
}

bool HirariUnifiedEngine::move_midi_notes_range(uint32_t trackId, uint64_t startSample,
                                              uint64_t endSample, int64_t deltaSamples) {
    return hirari_midi_notes_edit_range(m_midiSnapshotStore, trackId, startSample, endSample,
                                        2, deltaSamples, undo_state(), log_state());
}

void HirariUnifiedEngine::set_midi_note(uint32_t trackId, uint8_t pitch, uint8_t velocity,
                                      uint64_t startSample, uint64_t lengthSamples) {
    hirari_midi_notes_upsert(m_midiSnapshotStore, trackId, pitch, velocity, startSample,
                             lengthSamples, undo_state(), log_state());
}

void HirariUnifiedEngine::set_playing(bool p) {
    m_isPlaying.store(p, std::memory_order_release);
    if (!p) {
        // Voice state is owned by the callback. Ask that thread to release
        // preview/fallback voices on its next block instead of racing it here.
        m_previewVoicesNeedReset.store(true, std::memory_order_release);
        m_midiPanicRequested.store(true, std::memory_order_release);
        // The callback is quiescent once transport is stopped. Reclaim old
        // immutable region generations on the control thread only.
        waitForAudioCallbacks();
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            track->collectRetiredRegionSnapshots();
        }
    }
}
bool HirariUnifiedEngine::is_playing() const { return m_isPlaying.load(); }
uint32_t HirariUnifiedEngine::get_block_size() const noexcept {
    return m_context ? m_context->blockSize : 0u;
}
float HirariUnifiedEngine::get_latency_ms() const noexcept {
    uint32_t masterLatency = get_block_size();
    if (m_masterLimiter) {
        masterLatency += m_masterLimiter->getLatencySamples();
    }
    return hirari_samples_to_milliseconds(
        masterLatency, m_sampleRate.load(std::memory_order_relaxed));
}

float HirariUnifiedEngine::get_track_latency_ms(uint32_t tid) const noexcept {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    const Track* track = findTrackLocked(tid);
    return track ? hirari_samples_to_milliseconds(track->getTotalLatencySamples(), sampleRate)
                 : 0.0f;
}

float HirariUnifiedEngine::get_track_pdc_compensation_ms(uint32_t tid) const noexcept {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    const Track* track = findTrackLocked(tid);
    return track ? hirari_samples_to_milliseconds(track->getPdcCompensationSamples(), sampleRate)
                 : 0.0f;
}

bool HirariUnifiedEngine::set_low_latency_mode(bool active) noexcept {
    return hirari_native_pdc_set_low_latency_mode(m_pdcState, active);
}

bool HirariUnifiedEngine::low_latency_mode() const noexcept {
    return hirari_native_pdc_low_latency_mode(m_pdcState);
}

void HirariUnifiedEngine::set_playhead(uint64_t p) { m_playhead.store(p); }
uint64_t HirariUnifiedEngine::get_playhead() const { return m_playhead.load(); }
void HirariUnifiedEngine::set_loop(bool enabled) { m_loopEnabled.store(enabled); }
bool HirariUnifiedEngine::set_cycle_range(uint64_t start_sample, uint64_t end_sample, bool enabled) {
    if (start_sample >= end_sample) return false;
    m_cycleStart.store(start_sample, std::memory_order_release);
    m_cycleEnd.store(end_sample, std::memory_order_release);
    m_loopEnabled.store(enabled, std::memory_order_release);
    return true;
}
bool HirariUnifiedEngine::is_loop_enabled() const noexcept {
    return m_loopEnabled.load(std::memory_order_acquire);
}
uint64_t HirariUnifiedEngine::cycle_start() const noexcept { return m_cycleStart.load(std::memory_order_acquire); }
uint64_t HirariUnifiedEngine::cycle_end() const noexcept { return m_cycleEnd.load(std::memory_order_acquire); }
void HirariUnifiedEngine::set_metronome_enabled(bool enabled) noexcept {
    hirari_metronome_set_enabled(m_metronome, enabled);
}
bool HirariUnifiedEngine::is_metronome_enabled() const noexcept {
    return hirari_metronome_is_enabled(m_metronome);
}
void HirariUnifiedEngine::set_test_tone(bool enabled) { m_testTone.store(enabled, std::memory_order_release); }

uint32_t HirariUnifiedEngine::allocateTrackIdLocked() noexcept {
    uint32_t candidate = m_trackIdCounter.load(std::memory_order_relaxed);
    if (candidate == 0) candidate = 1;
    for (uint64_t attempts = 0; attempts < std::numeric_limits<uint32_t>::max(); ++attempts) {
        const bool used = std::any_of(m_tracks.begin(), m_tracks.end(),
            [candidate](const auto& track) { return track && track->getId() == candidate; });
        if (!used && candidate != 0 && candidate != std::numeric_limits<uint32_t>::max()) {
            m_trackIdCounter.store(candidate + 1u, std::memory_order_relaxed);
            return candidate;
        }
        candidate = candidate == std::numeric_limits<uint32_t>::max() - 1u ? 1u : candidate + 1u;
    }
    return 0;
}

uint32_t HirariUnifiedEngine::allocateRegionIdLocked() noexcept {
    uint32_t candidate = m_regionIdCounter.load(std::memory_order_relaxed);
    if (candidate == 0) candidate = 1;
    for (uint64_t attempts = 0; attempts < std::numeric_limits<uint32_t>::max(); ++attempts) {
        bool used = false;
        for (const auto& track : m_tracks) {
            if (!track) continue;
            for (const auto& region : track->getRegions()) {
                if (region.id == candidate) { used = true; break; }
            }
            if (used) break;
        }
        if (!used && candidate != std::numeric_limits<uint32_t>::max()) {
            m_regionIdCounter.store(candidate + 1u, std::memory_order_relaxed);
            return candidate;
        }
        candidate = candidate == std::numeric_limits<uint32_t>::max() - 1u ? 1u : candidate + 1u;
    }
    return 0;
}


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
    m_inputMonitorTrackId.store(kNoInputMonitorTrack, std::memory_order_release);
    m_inputMonitorBlockPublished.store(false, std::memory_order_release);
    m_commandQueue.clear();
    hirari_region_jobs_clear(this);
    set_test_tone(false);
    set_playhead(0);
    hirari_mpe_state_reset(m_mpeState);
    hirari_mpe_manager_reset(m_mpeManager);
    m_undoManager.clear();
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
    // the previous active-state pointers; destruction is deferred safely.
    m_deferredDestructionTracks.insert(m_deferredDestructionTracks.end(),
                                       std::make_move_iterator(m_tracks.begin()),
                                       std::make_move_iterator(m_tracks.end()));
    m_tracks.clear();
    mark_project_layout_changed();
    m_regionIdCounter.store(1, std::memory_order_release);
    // Track ID 0 is the invalid/stale-ID sentinel across the whole bridge:
    // ProjectDocument::validate() rejects it outright and the argument guards
    // treat it as "no track", so a starter track at ID 0 could never be
    // persisted or addressed. The starter therefore takes ID 1 (reserved for
    // it while the session is fresh) and user-created tracks continue at 2.
    m_trackIdCounter.store(2, std::memory_order_release);
    // Start with one audio track so a new project is immediately usable for
    // importing or recording, instead of presenting a dead empty canvas.
    auto bootstrapTrack = std::make_shared<Track>(1, "Audio 1", Track::Audio,
                                                  m_sidechainEngine, m_projectLayoutRevision);
    if (m_context) {
        bootstrapTrack->prepareToPlay(m_context->sampleRate, m_context->blockSize);
    }
    m_tracks.push_back(std::move(bootstrapTrack));
    mark_project_layout_changed();
    publishTrackSnapshotLocked(true);
}

