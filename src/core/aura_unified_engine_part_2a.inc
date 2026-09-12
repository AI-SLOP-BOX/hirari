void AuraUnifiedEngine::waitForAudioCallbacks() const noexcept {
    // This is a control/shutdown path, never the real-time callback. Yielding
    // avoids destroying Track objects while a callback still owns raw state
    // pointers, without introducing a mutex into the audio path.
    while (m_audioCallbacksInFlight.load(std::memory_order_acquire) != 0) {
        std::this_thread::yield();
    }
}

void AuraUnifiedEngine::set_preview_sample(const float* samples, size_t count, double sourceRate) {
    if (samples == nullptr || count == 0 || !std::isfinite(sourceRate) || sourceRate <= 0.0) {
        clear_preview_sample();
        return;
    }

    // Reclaim superseded snapshots only while the callback is quiescent. While
    // playing, old snapshots remain alive so the RT thread never dereferences
    // freed sample memory.
    if (!m_isPlaying.load(std::memory_order_acquire)) {
        m_previewSample.store(nullptr, std::memory_order_release);
        waitForAudioCallbacks();
        m_previewSampleStorage.clear();
    }
    auto snapshot = std::make_unique<PreviewSample>();
    snapshot->samples.assign(samples, samples + count);
    snapshot->sourceRate = sourceRate;
    const PreviewSample* published = snapshot.get();
    m_previewSampleStorage.push_back(std::move(snapshot));
    m_previewSample.store(published, std::memory_order_release);
}

void AuraUnifiedEngine::trigger_preview_sample() {
    m_previewSampleTrigger.store(true, std::memory_order_release);
}

void AuraUnifiedEngine::clear_preview_sample() {
    m_previewSample.store(nullptr, std::memory_order_release);
    if (!m_isPlaying.load(std::memory_order_acquire)) {
        waitForAudioCallbacks();
        m_previewSampleStorage.clear();
    }
    m_previewSampleTrigger.store(false, std::memory_order_release);
    m_previewSamplePlaying.store(false, std::memory_order_release);
}

void AuraUnifiedEngine::clear_midi_notes() {
    replace_midi_notes({}, true);
}

bool AuraUnifiedEngine::replace_midi_notes(const std::vector<ScheduledMidiNote>& notes, bool recordUndo) {
    if (notes.size() > 100000) return false;
    for (const auto& note : notes) {
        if (note.pitch > 127 || note.velocity == 0 ||
            note.lengthSamples == 0 || note.startSample > UINT64_MAX - note.lengthSamples) {
            return false;
        }
    }

    std::vector<ScheduledMidiNote> before;
    std::vector<ScheduledMidiNote> after = notes;
    {
        std::lock_guard<std::mutex> lock(m_midiNotesMutex);
        const uint32_t active = m_activeMidiSnapshot.load(std::memory_order_relaxed);
        before = m_midiNoteSnapshots[active];
        m_midiNoteSnapshots[1u - active] = after;
        m_activeMidiSnapshot.store(1u - active, std::memory_order_release);
    }

    const auto publish = [this](const std::vector<ScheduledMidiNote>& snapshot) {
        std::lock_guard<std::mutex> lock(m_midiNotesMutex);
        const uint32_t active = m_activeMidiSnapshot.load(std::memory_order_relaxed);
        m_midiNoteSnapshots[1u - active] = snapshot;
        m_activeMidiSnapshot.store(1u - active, std::memory_order_release);
    };
    if (recordUndo) {
        m_undoManager.recordAppliedAction(
            "Replace MIDI Notes",
            [publish, before]() { publish(before); },
            [publish, after]() { publish(after); });
    }
    return true;
}

std::vector<AuraUnifiedEngine::ScheduledMidiNote> AuraUnifiedEngine::midi_notes_snapshot() const {
    std::lock_guard<std::mutex> lock(m_midiNotesMutex);
    const uint32_t active = m_activeMidiSnapshot.load(std::memory_order_acquire);
    return m_midiNoteSnapshots[active];
}

bool AuraUnifiedEngine::remove_midi_notes_range(uint32_t trackId, uint64_t startSample,
                                                uint64_t endSample) {
    if (trackId == 0 || startSample >= endSample) return false;
    auto next = midi_notes_snapshot();
    const auto before = next.size();
    next.erase(std::remove_if(next.begin(), next.end(), [&](const ScheduledMidiNote& note) {
        const uint64_t noteEnd = note.startSample > UINT64_MAX - note.lengthSamples
            ? UINT64_MAX : note.startSample + note.lengthSamples;
        return note.trackId == trackId
            && note.startSample < endSample
            && noteEnd > startSample;
    }), next.end());
    if (next.size() == before) return false;
    return replace_midi_notes(next, true);
}

bool AuraUnifiedEngine::transpose_midi_notes_range(uint32_t trackId, uint64_t startSample,
                                                   uint64_t endSample, int32_t semitones) {
    if (trackId == 0 || startSample >= endSample || semitones < -127 || semitones > 127) return false;
    auto next = midi_notes_snapshot();
    bool changed = false;
    for (auto& note : next) {
        const uint64_t noteEnd = note.startSample > UINT64_MAX - note.lengthSamples
            ? UINT64_MAX : note.startSample + note.lengthSamples;
        if (note.trackId != trackId || note.startSample >= endSample || noteEnd <= startSample) continue;
        const int32_t pitch = static_cast<int32_t>(note.pitch) + semitones;
        if (pitch < 0 || pitch > 127) return false;
        note.pitch = static_cast<uint8_t>(pitch);
        changed = true;
    }
    if (!changed) return false;
    return replace_midi_notes(next, true);
}

bool AuraUnifiedEngine::move_midi_notes_range(uint32_t trackId, uint64_t startSample,
                                              uint64_t endSample, int64_t deltaSamples) {
    if (trackId == 0 || startSample >= endSample) return false;
    auto next = midi_notes_snapshot();
    bool changed = false;
    for (auto& note : next) {
        const uint64_t noteEnd = note.startSample > UINT64_MAX - note.lengthSamples
            ? UINT64_MAX : note.startSample + note.lengthSamples;
        if (note.trackId != trackId || note.startSample >= endSample || noteEnd <= startSample) continue;
        if (deltaSamples < 0) {
            const uint64_t distance = static_cast<uint64_t>(-(deltaSamples + 1)) + 1;
            if (note.startSample < distance) return false;
            note.startSample -= distance;
        } else {
            const uint64_t distance = static_cast<uint64_t>(deltaSamples);
            if (note.startSample > UINT64_MAX - distance) return false;
            note.startSample += distance;
        }
        changed = true;
    }
    if (!changed) return false;
    return replace_midi_notes(next, true);
}

void AuraUnifiedEngine::set_midi_note(uint32_t trackId, uint8_t pitch, uint8_t velocity,
                                      uint64_t startSample, uint64_t lengthSamples) {
    if (pitch > 127 || velocity == 0 || lengthSamples == 0 ||
        startSample > UINT64_MAX - lengthSamples) return;
    auto next = midi_notes_snapshot();
    auto existing = std::find_if(next.begin(), next.end(), [&](const ScheduledMidiNote& note) {
        return note.trackId == trackId && note.pitch == pitch && note.startSample == startSample;
    });
    if (existing != next.end()) {
        if (existing->velocity == velocity && existing->lengthSamples == lengthSamples) return;
        existing->velocity = velocity;
        existing->lengthSamples = lengthSamples;
    } else {
        next.push_back(ScheduledMidiNote{trackId, pitch, velocity, startSample, lengthSamples});
    }
    replace_midi_notes(next, true);
}

void AuraUnifiedEngine::set_playing(bool p) {
    m_isPlaying.store(p, std::memory_order_release);
    if (!p) {
        // The callback is quiescent once transport is stopped. Reclaim old
        // immutable region generations on the control thread only.
        waitForAudioCallbacks();
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            track->collectRetiredRegionSnapshots();
        }
    }
}
bool AuraUnifiedEngine::is_playing() const { return m_isPlaying.load(); }
uint32_t AuraUnifiedEngine::get_block_size() const noexcept {
    return m_context ? m_context->blockSize : 0u;
}
float AuraUnifiedEngine::get_latency_ms() const noexcept {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    const uint32_t blockSize = get_block_size();
    if (!std::isfinite(sampleRate) || sampleRate <= 0.0 || blockSize == 0) return 0.0f;
    uint32_t masterLatency = blockSize;
    if (m_masterLimiter) {
        masterLatency += m_masterLimiter->getLatencySamples();
    }
    return static_cast<float>(1000.0 * static_cast<double>(masterLatency) / sampleRate);
}

float AuraUnifiedEngine::get_master_tail_ms() const noexcept {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    if (!std::isfinite(sampleRate) || sampleRate <= 0.0 || !m_masterLimiter) return 0.0f;
    return static_cast<float>(static_cast<double>(m_masterLimiter->getTailSamples()) *
                              1000.0 / sampleRate);
}

float AuraUnifiedEngine::get_track_latency_ms(uint32_t tid) const noexcept {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    if (!std::isfinite(sampleRate) || sampleRate <= 0.0) return 0.0f;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track->getId() == tid) {
            return static_cast<float>(
                static_cast<double>(track->getTotalLatencySamples()) * 1000.0 / sampleRate);
        }
    }
    return 0.0f;
}

float AuraUnifiedEngine::get_track_tail_ms(uint32_t tid) const noexcept {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    if (!std::isfinite(sampleRate) || sampleRate <= 0.0) return 0.0f;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid) {
            return static_cast<float>(static_cast<double>(track->getTotalTailSamples()) *
                                      1000.0 / sampleRate);
        }
    }
    return 0.0f;
}

float AuraUnifiedEngine::get_track_pdc_compensation_ms(uint32_t tid) const noexcept {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    if (!std::isfinite(sampleRate) || sampleRate <= 0.0) return 0.0f;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track->getId() == tid) {
            return static_cast<float>(
                static_cast<double>(track->getPdcCompensationSamples()) * 1000.0 / sampleRate);
        }
    }
    return 0.0f;
}

bool AuraUnifiedEngine::set_low_latency_mode(bool active) noexcept {
    return m_pdcManager.setLowLatencyMode(active);
}

bool AuraUnifiedEngine::low_latency_mode() const noexcept {
    return m_pdcManager.lowLatencyMode();
}

bool AuraUnifiedEngine::set_tonal_scale(int32_t root, uint32_t scale_type) noexcept {
    if (scale_type > 10) return false;
    using ScaleType = Aura::Core::Engine::TonalSync::ScaleType;
    static constexpr ScaleType kScales[] = {
        ScaleType::Major, ScaleType::Minor, ScaleType::HarmonicMinor,
        ScaleType::MelodicMinor, ScaleType::Dorian, ScaleType::Phrygian,
        ScaleType::Lydian, ScaleType::Mixolydian, ScaleType::Aeolian,
        ScaleType::Locrian, ScaleType::PentatonicMajor,
    };
    m_tonalSync.setScale(root, kScales[scale_type]);
    return true;
}

bool AuraUnifiedEngine::is_note_in_tonal_scale(int32_t midi_note) const noexcept {
    return m_tonalSync.isNoteInScale(midi_note);
}

int32_t AuraUnifiedEngine::tonal_root() const noexcept { return m_tonalSync.root(); }
uint32_t AuraUnifiedEngine::tonal_scale_type() const noexcept { return m_tonalSync.scaleType(); }
void AuraUnifiedEngine::set_playhead(uint64_t p) { m_playhead.store(p); }
uint64_t AuraUnifiedEngine::get_playhead() const { return m_playhead.load(); }
void AuraUnifiedEngine::set_loop(bool enabled) { m_loopEnabled.store(enabled); }
bool AuraUnifiedEngine::set_cycle_range(uint64_t start_sample, uint64_t end_sample, bool enabled) {
    if (start_sample >= end_sample) return false;
    m_cycleStart.store(start_sample, std::memory_order_release);
    m_cycleEnd.store(end_sample, std::memory_order_release);
    m_loopEnabled.store(enabled, std::memory_order_release);
    return true;
}
bool AuraUnifiedEngine::is_loop_enabled() const noexcept {
    return m_loopEnabled.load(std::memory_order_acquire);
}
uint64_t AuraUnifiedEngine::cycle_start() const noexcept { return m_cycleStart.load(std::memory_order_acquire); }
uint64_t AuraUnifiedEngine::cycle_end() const noexcept { return m_cycleEnd.load(std::memory_order_acquire); }
void AuraUnifiedEngine::set_metronome_enabled(bool enabled) noexcept {
    m_metronome.setEnabled(enabled);
}
bool AuraUnifiedEngine::is_metronome_enabled() const noexcept {
    return m_metronome.isEnabled();
}
void AuraUnifiedEngine::set_test_tone(bool enabled) { m_testTone.store(enabled, std::memory_order_release); }

uint32_t AuraUnifiedEngine::allocateTrackIdLocked() noexcept {
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

uint32_t AuraUnifiedEngine::allocateRegionIdLocked() noexcept {
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

uint32_t AuraUnifiedEngine::add_track(uint32_t type) {
    std::shared_ptr<Track> created;
    uint32_t id = 0;
    size_t insertIndex = 0;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        id = allocateTrackIdLocked();
        if (id == 0) return 0;
        const auto trackType = static_cast<Track::Type>(type);
        if (trackType == Track::Type::Bus) {
            auto busTrack = std::make_shared<BusTrack>(id, "Bus " + std::to_string(id + 1), id,
                                                       &m_sidechainManager);
            busTrack->resolveBus(m_busSystem.registerBus(id));
            created = std::move(busTrack);
        } else {
            created = std::make_shared<Track>(id, "Track " + std::to_string(id + 1), trackType,
                                              &m_sidechainManager);
        }
        if (m_context) created->prepareToPlay(m_context->sampleRate, m_context->blockSize);
        insertIndex = m_tracks.size();
        m_tracks.push_back(created);

        uint32_t nextIdx = 1 - m_activeStateIdx.load(std::memory_order_acquire);
        auto& state = m_audioStates[nextIdx];
        state.audioConfigGeneration = m_audioConfigGeneration.load(std::memory_order_acquire);
        state.trackCount = 0;
        for (const auto& trk : m_tracks) {
            if (state.trackCount < kMaxTracks) state.activeTracks[state.trackCount++] = trk.get();
        }
        m_activeStateIdx.store(nextIdx, std::memory_order_release);
    }

    const auto removeCreated = [this, id]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        m_sidechainManager.removeLinksForTrack(id);
        auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
                               [id](const auto& track) { return track->getId() == id; });
        if (it == m_tracks.end()) return;
        {
            std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
            for (const auto& region : (*it)->getRegions()) m_peakCache.erase(region.id);
        }
        m_deferredDestructionTracks.push_back(*it);
        m_tracks.erase(it);
        uint32_t nextIdx = 1 - m_activeStateIdx.load(std::memory_order_acquire);
        auto& state = m_audioStates[nextIdx];
        state.trackCount = 0;
        for (const auto& track : m_tracks) {
            if (state.trackCount < kMaxTracks) state.activeTracks[state.trackCount++] = track.get();
        }
        m_activeStateIdx.store(nextIdx, std::memory_order_release);
    };
    const auto restoreCreated = [this, created, id, insertIndex]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        const bool alreadyPresent = std::any_of(m_tracks.begin(), m_tracks.end(),
            [id](const auto& track) { return track->getId() == id; });
        if (alreadyPresent) return;
        m_tracks.insert(m_tracks.begin() + std::min(insertIndex, m_tracks.size()), created);
        uint32_t nextIdx = 1 - m_activeStateIdx.load(std::memory_order_acquire);
        auto& state = m_audioStates[nextIdx];
        state.audioConfigGeneration = m_audioConfigGeneration.load(std::memory_order_acquire);
        state.trackCount = 0;
        for (const auto& track : m_tracks) {
            if (state.trackCount < kMaxTracks) state.activeTracks[state.trackCount++] = track.get();
        }
        m_activeStateIdx.store(nextIdx, std::memory_order_release);
    };
    m_undoManager.recordAppliedAction("Add Track", removeCreated, restoreCreated);
    return id;
}

bool AuraUnifiedEngine::add_vca_group(uint32_t groupId, float gain) {
    if (groupId == 0 || !std::isfinite(gain) || gain < 0.0f || gain > 8.0f) return false;
    auto& vca = VCAManager::getInstance();
    vca.addGroup(groupId, gain);
    vca.resolveHierarchy();
    return true;
}

bool AuraUnifiedEngine::assign_track_to_vca(uint32_t trackId, uint32_t groupId) {
    if (trackId == 0 || groupId == 0) return false;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        const bool exists = std::any_of(m_tracks.begin(), m_tracks.end(),
            [trackId](const auto& track) { return track && track->getId() == trackId; });
        if (!exists) return false;
    }
    return VCAManager::getInstance().assignTrack(trackId, groupId);
}

bool AuraUnifiedEngine::set_vca_group_gain(uint32_t groupId, float gain) {
    if (groupId == 0 || !std::isfinite(gain) || gain < 0.0f || gain > 8.0f) return false;
    auto& vca = VCAManager::getInstance();
    vca.addGroup(groupId, gain);
    vca.resolveHierarchy();
    return true;
}

float AuraUnifiedEngine::get_vca_track_gain(uint32_t trackId) const {
    return VCAManager::getInstance().getCumulativeGain(trackId);
}

rust::String AuraUnifiedEngine::get_vca_snapshot_json() const {
    const auto groups = VCAManager::getInstance().snapshotGroups();
    std::ostringstream stream;
    stream << "[";
    for (size_t index = 0; index < groups.size(); ++index) {
        if (index > 0) stream << ",";
        const auto& group = groups[index];
        stream << "{\"id\":" << group.id
               << ",\"gain\":" << (std::isfinite(group.gain) ? group.gain : 1.0f)
               << ",\"track_ids\":[";
        for (size_t member = 0; member < group.trackIds.size(); ++member) {
            if (member > 0) stream << ",";
            stream << group.trackIds[member];
        }
        stream << "]}";
    }
    stream << "]";
    return rust::String(stream.str());
}

void AuraUnifiedEngine::clear_vca_groups() {
    VCAManager::getInstance().clear();
}

void AuraUnifiedEngine::new_project() {
    set_playing(false);
    waitForAudioCallbacks();
    // Input monitoring is a live device assignment, not project data. Clear
    // it before reusing any track IDs in the new project.
    m_inputMonitorTrackId.store(kNoInputMonitorTrack, std::memory_order_release);
    m_inputMonitorBlockPublished.store(false, std::memory_order_release);
    m_commandQueue.clear();
    set_test_tone(false);
    set_playhead(0);
    m_midiOrchestrator.reset();
    MPEManager::getInstance().reset();
    m_undoManager.clear();
    m_pdcManager.resetForProject();
    m_sidechainManager.resetForProject();
    m_routingEngine.resetForProject();
    m_controlRoom.resetForProject();
    m_tempo.store(120.0f, std::memory_order_release);
    m_masterGain.store(1.0f, std::memory_order_release);
    m_tonalSync.setScale(0, Aura::Core::Engine::TonalSync::ScaleType::Major);
    m_waveformGeneration.fetch_add(1, std::memory_order_acq_rel);
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    m_sidechainRoutes.clear();
    // Keep old Track objects alive while a CoreAudio callback may still hold
    // the previous active-state pointers; destruction is deferred safely.
    m_deferredDestructionTracks.insert(m_deferredDestructionTracks.end(),
                                       std::make_move_iterator(m_tracks.begin()),
                                       std::make_move_iterator(m_tracks.end()));
    m_tracks.clear();
    {
        std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
        m_peakCache.clear();
        // Do not let a worker from the previous project generation suppress
        // the first waveform request for a newly loaded project. Its stale
        // completion is still harmless because the worker checks its captured
        // generation before publishing.
        m_peakBuilds.clear();
    }
    m_regionIdCounter.store(1, std::memory_order_release);
    // A freshly created template intentionally starts with track ID 0 for
    // compatibility with the public project/template format. User-created
    // tracks continue at 1, so zero remains unique to the starter track.
    m_trackIdCounter.store(1, std::memory_order_release);
    // Start with one audio track so a new project is immediately usable for
    // importing or recording, instead of presenting a dead empty canvas.
    auto bootstrapTrack = std::make_shared<Track>(0, "Audio 1", Track::Audio,
                                                  &m_sidechainManager);
    if (m_context) {
        bootstrapTrack->prepareToPlay(m_context->sampleRate, m_context->blockSize);
    }
    m_tracks.push_back(std::move(bootstrapTrack));
    uint32_t nextIdx = 1 - m_activeStateIdx.load(std::memory_order_acquire);
    m_audioStates[nextIdx].audioConfigGeneration =
        m_audioConfigGeneration.load(std::memory_order_acquire);
    m_audioStates[nextIdx].trackCount = 1;
    m_audioStates[nextIdx].activeTracks[0] = m_tracks.front().get();
    m_activeStateIdx.store(nextIdx, std::memory_order_release);
}

uint32_t AuraUnifiedEngine::duplicate_track(uint32_t tid) {
    std::shared_ptr<Track> copy;
    size_t insertIndex = 0;
    uint32_t newId = 0;
    std::vector<uint32_t> pluginTypes;
    std::vector<std::vector<uint8_t>> pluginStates;
    std::vector<bool> pluginBypasses;
    std::vector<std::string> sandboxPaths;
    std::vector<ProjectSerializer::AutomationPoint> volumeAutomation;
    std::vector<ProjectSerializer::AutomationPoint> panAutomation;
    std::vector<ProjectSerializer::AutomationPoint> delayAutomation;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        auto source = std::find_if(m_tracks.begin(), m_tracks.end(),
                                   [tid](const auto& track) { return track->getId() == tid; });
        if (source == m_tracks.end()) return 0;
        const auto& original = *source;
        pluginTypes = original->getPluginTypes();
        pluginStates = original->getPluginStates();
        volumeAutomation = original->getVolumeAutomationPoints();
        panAutomation = original->getPanAutomationPoints();
        delayAutomation = original->getTrackDelayAutomationPoints();
        pluginBypasses.reserve(pluginTypes.size());
        for (size_t index = 0; index < pluginTypes.size(); ++index) {
            pluginBypasses.push_back(original->getPluginBypass(static_cast<uint32_t>(index)));
        }
        sandboxPaths = original->getSandboxedPluginPaths();
        newId = allocateTrackIdLocked();
        if (newId == 0) return 0;
        if (original->getType() == Track::Type::Bus) {
            auto busCopy = std::make_shared<BusTrack>(newId, original->getName() + " Copy", newId,
                                                      &m_sidechainManager);
            busCopy->resolveBus(m_busSystem.registerBus(newId));
            copy = std::move(busCopy);
        } else {
            copy = std::make_shared<Track>(newId, original->getName() + " Copy",
                                           original->getType(), &m_sidechainManager);
        }
        copy->setVolume(original->getVolume());
        copy->setPan(original->getPan());
        copy->setMuted(original->isMuted());
        copy->setPhaseInverted(original->isPhaseInverted());
        copy->setVolumeAutomation(volumeAutomation);
        copy->setPanAutomation(panAutomation);
        copy->setTrackDelayAutomation(delayAutomation);
        size_t sandboxPathIndex = 0;
        for (size_t pluginIndex = 0; pluginIndex < pluginTypes.size(); ++pluginIndex) {
            const uint32_t pluginType = pluginTypes[pluginIndex];
            bool inserted = false;
            if (pluginType == 0xFFFFFFFFu) {
                if (sandboxPathIndex < sandboxPaths.size()) {
                    inserted = copy->addSandboxedPlugin(sandboxPaths[sandboxPathIndex]);
                }
                ++sandboxPathIndex;
            } else {
                inserted = copy->addPlugin(pluginType);
            }
            if (inserted && pluginIndex < pluginStates.size() && !pluginStates[pluginIndex].empty()) {
                const uint32_t destinationIndex = static_cast<uint32_t>(copy->getPluginCount() - 1);
                copy->setPluginState(destinationIndex, pluginStates[pluginIndex]);
                if (pluginIndex < pluginBypasses.size()) {
                    copy->setPluginBypass(destinationIndex, pluginBypasses[pluginIndex]);
                }
            }
        }
        for (const auto& region : original->getRegions()) {
            Region cloned = region;
            cloned.id = allocateRegionIdLocked();
            if (cloned.id == 0) return 0;
            copy->addRegion(cloned);
        }
        // MIDI regions are independent objects and cannot be shallow-copied
        // because their note vectors are mutex-protected. Clone the notes
        // through the safe control-plane accessor and allocate fresh IDs.
        for (const auto& midiRegion : original->getMidiRegions()) {
            if (!midiRegion) continue;
            std::vector<MIDINote> notes;
            if (!midiRegion->copyProcessedNotes(notes)) return 0;
            const uint32_t midiId = allocateRegionIdLocked();
            if (midiId == 0) return 0;
            auto midiCopy = std::make_shared<Aura::Core::MidiRegion>(
                midiId, midiRegion->getName(), midiRegion->getStartBeat(),
                midiRegion->getLengthBeats());
            if (!midiCopy->replaceNotes(notes)) return 0;
            copy->addMidiRegion(std::move(midiCopy));
        }
        insertIndex = m_tracks.size();
        m_tracks.push_back(copy);
        uint32_t nextIdx = 1 - m_activeStateIdx.load(std::memory_order_acquire);
        auto& state = m_audioStates[nextIdx];
        state.trackCount = 0;
        for (const auto& track : m_tracks) {
