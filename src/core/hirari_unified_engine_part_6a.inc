bool HirariUnifiedEngine::set_track_eq(uint32_t trackId, float lowBoostDb, float lowCutDb,
                                     float highBoostDb, float highCutDb) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track->getId() == trackId) {
            track->setEq(lowBoostDb, lowCutDb, highBoostDb, highCutDb);
            return true;
        }
    }
    return false;
}

// History control (undo, redo, transactions, counts) is Rust-owned; the C++
// facade calls the Rust entry points directly. The one thing that must stay
// here is the graph resync that follows a replayed structural edit.
extern "C" void hirari_engine_sync_structural_changes(void* host) {
    if (host) static_cast<HirariUnifiedEngine*>(host)->syncStructuralChanges();
}

rust::String HirariUnifiedEngine::get_project_layout_json() const {
    return get_project_layout_json_cancellable(nullptr);
}

rust::String HirariUnifiedEngine::get_project_layout_json_cancellable(
    const std::atomic<bool>* cancellation) const {
    // The serializer is Rust-owned. This function only gathers the current
    // model into a borrowed snapshot and keeps it alive (and stable) for the
    // synchronous call. Every ordering, escaping and formatting rule lives in
    // hirari-core-bridge/src/project_layout.rs.
    const auto cancelled = [cancellation]() noexcept {
        return cancellation && cancellation->load(std::memory_order_acquire);
    };
    if (cancelled()) return rust::String();
    if (m_ecoModeLogTriggered.exchange(false, std::memory_order_acq_rel)) {
        ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0, "SYSTEM | GOVERNOR | ECO_MODE_ACTIVE");
    }
    if (m_videoSyncLogTriggered.exchange(false, std::memory_order_acq_rel)) {
        ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0, "SYSTEM | VIDEO | SYNC_LOCKED");
    }

    static_assert(sizeof(Hirari::Core::ProjectAutomationPoint) ==
                  sizeof(HirariLayoutAutomationPoint));
    static_assert(sizeof(Region::RangeEdit) == sizeof(HirariRegionRangeEdit));

    // Serialization borrows data owned by each track. Heap-allocate every
    // per-track block: moving it would invalidate the pointers to short-string
    // buffers that the snapshot already holds.
    struct TrackLayoutStorage {
        HirariLayoutTrack view{};
        std::string name;
        std::string frozenPath;
        std::string expressionMap;
        std::string expressionMapPro;
        std::vector<HirariLayoutAutomationPoint> volumeAutomation;
        std::vector<HirariLayoutAutomationPoint> panAutomation;
        std::vector<HirariLayoutAutomationPoint> trackDelayAutomation;
        std::vector<HirariLayoutPluginLane> pluginAutomation;
        std::vector<std::vector<Hirari::Core::ProjectAutomationPoint>> pluginAutomationPoints;
        std::vector<uint8_t> pluginBypass;
        std::vector<std::vector<float>> pluginParameters;
        std::vector<HirariLayoutParameterBlock> pluginParameterBlocks;
        std::vector<std::vector<uint8_t>> pluginStates;
        std::vector<HirariLayoutByteBlock> pluginStateBlocks;
        std::vector<HirariLayoutString> sandboxPaths;
        std::vector<std::vector<uint8_t>> sandboxStates;
        std::vector<HirariLayoutByteBlock> sandboxStateBlocks;
        std::vector<HirariLayoutRegion> regions;
    };

    std::vector<Hirari::Core::Engine::RoutingEngine::FeedbackConnection> feedbackConnections;
    m_routingEngine.copyFeedbackConnections(feedbackConnections);

    const double timelineSampleRate = m_sampleRate.load(std::memory_order_relaxed);
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (cancelled()) return rust::String();
        if (!track) continue;
        // A callback may have queued write-automation points since the
        // previous snapshot. Drain them on this control-side boundary so UI,
        // CLI generation checks, history diffs, and project saves all observe
        // the same automation state.
        track->flushRecordedAutomation();
        track->reclaimEffectChainRetiredMemory();
    }

    const auto layoutString = [](const std::string& value) {
        HirariLayoutString out;
        out.data = reinterpret_cast<const uint8_t*>(value.data());
        out.len = value.size();
        return out;
    };
    const auto layoutPoints = [](const std::vector<Hirari::Core::ProjectAutomationPoint>& input) {
        std::vector<HirariLayoutAutomationPoint> out(input.size());
        for (size_t index = 0; index < input.size(); ++index) {
            out[index] = {input[index].time, input[index].value, input[index].curve};
        }
        return out;
    };

    std::vector<std::unique_ptr<TrackLayoutStorage>> storages;
    std::vector<HirariLayoutTrack> trackViews;
    std::vector<HirariLayoutSidechainRoute> sidechainRoutes;
    std::vector<HirariLayoutFeedbackRoute> feedbackRoutes;
    storages.reserve(m_tracks.size());
    trackViews.reserve(m_tracks.size());
    for (const auto& track : m_tracks) {
        if (cancelled()) return rust::String();
        if (!track) continue;
        auto storage = std::make_unique<TrackLayoutStorage>();
        storage->name = track->getName();
        storage->frozenPath = track->frozenPath();
        storage->expressionMap = track->getExpressionMapJson();
        storage->expressionMapPro = track->getExpressionMapProJson();
        storage->volumeAutomation = layoutPoints(track->getVolumeAutomationPoints());
        storage->panAutomation = layoutPoints(track->getPanAutomationPoints());
        storage->trackDelayAutomation = layoutPoints(track->getTrackDelayAutomationPoints());

        const auto pluginAutomation = track->getPluginAutomation();
        storage->pluginAutomationPoints.reserve(pluginAutomation.size());
        for (const auto& lane : pluginAutomation)
            storage->pluginAutomationPoints.push_back(lane.points);
        storage->pluginAutomation.resize(pluginAutomation.size());
        for (size_t index = 0; index < pluginAutomation.size(); ++index) {
            auto& lane = storage->pluginAutomation[index];
            lane.pluginIndex = pluginAutomation[index].pluginIndex;
            lane.parameterId = pluginAutomation[index].parameterId;
            lane.points = reinterpret_cast<const HirariLayoutAutomationPoint*>(
                storage->pluginAutomationPoints[index].data());
            lane.pointCount = storage->pluginAutomationPoints[index].size();
        }

        const auto& pluginTypes = track->getPluginTypes();
        storage->pluginBypass.resize(pluginTypes.size());
        for (size_t index = 0; index < pluginTypes.size(); ++index) {
            storage->pluginBypass[index] =
                track->getPluginBypass(static_cast<uint32_t>(index)) ? 1 : 0;
        }
        storage->pluginParameters.reserve(pluginTypes.size());
        for (size_t index = 0; index < pluginTypes.size(); ++index) {
            if (cancelled()) return rust::String();
            const uint32_t parameterCount =
                track->getPluginParameterCount(static_cast<uint32_t>(index));
            std::vector<float> values(parameterCount);
            for (uint32_t parameter = 0; parameter < parameterCount; ++parameter) {
                values[parameter] =
                    track->getPluginParameter(static_cast<uint32_t>(index), parameter);
            }
            storage->pluginParameters.push_back(std::move(values));
        }
        storage->pluginParameterBlocks.resize(storage->pluginParameters.size());
        for (size_t index = 0; index < storage->pluginParameters.size(); ++index) {
            storage->pluginParameterBlocks[index] = {
                storage->pluginParameters[index].data(),
                storage->pluginParameters[index].size()};
        }

        storage->pluginStates = track->getPluginStates();
        storage->pluginStateBlocks.resize(storage->pluginStates.size());
        for (size_t index = 0; index < storage->pluginStates.size(); ++index) {
            storage->pluginStateBlocks[index] = {
                storage->pluginStates[index].data(), storage->pluginStates[index].size()};
        }

        const auto& sandboxPaths = track->getSandboxedPluginPaths();
        storage->sandboxPaths.resize(sandboxPaths.size());
        for (size_t index = 0; index < sandboxPaths.size(); ++index) {
            storage->sandboxPaths[index] = layoutString(sandboxPaths[index]);
        }
        storage->sandboxStates = track->getSandboxedPluginStates();
        storage->sandboxStateBlocks.resize(storage->sandboxStates.size());
        for (size_t index = 0; index < storage->sandboxStates.size(); ++index) {
            storage->sandboxStateBlocks[index] = {
                storage->sandboxStates[index].data(), storage->sandboxStates[index].size()};
        }

        const auto& regions = track->getRegions();
        storage->regions.resize(regions.size());
        for (size_t index = 0; index < regions.size(); ++index) {
            if (cancelled()) return rust::String();
            const auto& region = regions[index];
            auto& out = storage->regions[index];
            out.id = region.id;
            out.name = layoutString(region.name);
            out.state = region.editableState();
            out.sourceSampleRate = static_cast<uint32_t>(std::clamp(
                std::llround(region.sourceSampleRate > 0.0
                    ? region.sourceSampleRate : timelineSampleRate), 8000LL, 384000LL));
            out.path = layoutString(region.path);
            std::error_code pathError;
            out.missing = region.path.empty() ||
                !std::filesystem::exists(std::filesystem::path(region.path), pathError);
            // Distinguish a missing file from a file that exists but was not
            // decoded during project hydration; clients can surface a relink
            // or decoder error instead of silently showing an empty region.
            out.audioLoaded = region.audio != nullptr;
            out.warpMarkers = region.warpMarkers.data();
            out.warpMarkerCount = region.warpMarkers.size();
            out.rangeEdits =
                reinterpret_cast<const HirariRegionRangeEdit*>(region.rangeEdits.data());
            out.rangeEditCount = region.rangeEdits.size();
            out.noteStore = region.noteEdits.storeHandle();
            out.noteFrozen = region.noteEdits.frozenHandle();
            out.processingStore = region.processingHistory.storeHandle();
        }

        auto& view = storage->view;
        view.id = track->getId();
        view.name = layoutString(storage->name);
        view.type = static_cast<uint32_t>(track->getType());
        view.volume = track->getVolume();
        view.pan = track->getPan();
        view.muted = track->isMuted();
        view.solo = track->isSolo();
        view.recordArmed = track->isRecordArmed();
        view.phaseInverted = track->isPhaseInverted();
        view.trackDelaySamples = track->getTrackDelaySamples();
        view.frozen = track->isFrozen();
        view.frozenTotalSamples = track->frozenTotalSamples();
        view.frozenSampleRate = track->frozenSampleRate();
        view.frozenPath = layoutString(storage->frozenPath);
        view.volumeAutomation = storage->volumeAutomation.data();
        view.volumeAutomationCount = storage->volumeAutomation.size();
        view.panAutomation = storage->panAutomation.data();
        view.panAutomationCount = storage->panAutomation.size();
        view.trackDelayAutomation = storage->trackDelayAutomation.data();
        view.trackDelayAutomationCount = storage->trackDelayAutomation.size();
        view.expressionMapJson = layoutString(storage->expressionMap);
        view.expressionMapProJson = layoutString(storage->expressionMapPro);
        view.pluginAutomation = storage->pluginAutomation.data();
        view.pluginAutomationCount = storage->pluginAutomation.size();
        view.pluginTypes = pluginTypes.data();
        view.pluginTypeCount = pluginTypes.size();
        view.pluginBypass = storage->pluginBypass.data();
        view.pluginParameters = storage->pluginParameterBlocks.data();
        view.pluginStates = storage->pluginStateBlocks.data();
        view.pluginStateCount = storage->pluginStateBlocks.size();
        view.sandboxPluginPaths = storage->sandboxPaths.data();
        view.sandboxPluginPathCount = storage->sandboxPaths.size();
        view.sandboxPluginStates = storage->sandboxStateBlocks.data();
        view.sandboxPluginStateCount = storage->sandboxStateBlocks.size();
        view.regions = storage->regions.data();
        view.regionCount = storage->regions.size();

        trackViews.push_back(view);
        storages.push_back(std::move(storage));
    }

    sidechainRoutes.reserve(m_sidechainRoutes.size());
    for (const auto& route : m_sidechainRoutes) {
        HirariLayoutSidechainRoute out;
        out.sourceId = route.sourceId;
        out.destinationId = route.destinationId;
        out.pluginIndex = route.pluginIndex;
        out.tapPoint = route.tapPoint;
        sidechainRoutes.push_back(out);
    }
    feedbackRoutes.reserve(feedbackConnections.size());
    for (const auto& route : feedbackConnections) {
        feedbackRoutes.push_back({route.sourceId, route.destId, route.gain});
    }

    HirariProjectLayoutSnapshot snapshot{};
    snapshot.tracks = trackViews.data();
    snapshot.trackCount = trackViews.size();
    snapshot.sidechainRoutes = sidechainRoutes.data();
    snapshot.sidechainRouteCount = sidechainRoutes.size();
    snapshot.feedbackRoutes = feedbackRoutes.data();
    snapshot.feedbackRouteCount = feedbackRoutes.size();
    snapshot.timelineSampleRate = timelineSampleRate;

    const auto isCancelled = [](const void* context) -> bool {
        const auto* flag = static_cast<const std::atomic<bool>*>(context);
        return flag && flag->load(std::memory_order_acquire);
    };
    const uint8_t* jsonData = nullptr;
    size_t jsonSize = 0;
    if (!hirari_project_layout_write_json(&snapshot, cancellation ? isCancelled : nullptr,
                                          cancellation, &jsonData, &jsonSize)) {
        return rust::String();
    }
    rust::String result(jsonData ? reinterpret_cast<const char*>(jsonData) : "", jsonSize);
    hirari_project_layout_json_free(jsonData, jsonSize);
    return result;
}

rust::String HirariUnifiedEngine::get_routing_snapshot_json() const {
    return rust::String(m_routingEngine.getConnectionsJson());
}

uint64_t HirariUnifiedEngine::get_project_generation() const {
    const auto layout = get_project_layout_json();
    uint64_t hash = 0xcbf29ce484222325ull;
    hirari_project_layout_fingerprint_update(
        &hash, reinterpret_cast<const uint8_t*>(layout.data()), layout.size());
    return hash;
}

uint64_t HirariUnifiedEngine::get_project_layout_revision() const noexcept {
    return m_projectLayoutRevision ? m_projectLayoutRevision->current() : 0;
}

uint64_t HirariUnifiedEngine::get_project_state_revision() const noexcept {
    const uint64_t layout = get_project_layout_revision();
    const uint64_t midi = hirari_midi_snapshot_store_revision(m_midiSnapshotStore);
    const uint64_t control = m_projectControlRevision.load(std::memory_order_acquire);
    return hirari_project_state_revision_mix(layout, midi, control);
}

bool HirariUnifiedEngine::push_command(CommandType type, uint32_t tid, float val, uint64_t ts,
                                     uint64_t expectedProjectGeneration,
                                     uint64_t expectedAudioGeneration) {
    const auto setError = [this](CommandPushError error) {
        m_lastCommandError.store(static_cast<uint8_t>(error), std::memory_order_release);
    };
    // A bounded integer is not a valid target by itself.  Reject commands for
    // tracks that are no longer present instead of letting the legacy
    // get_track() fallback silently redirect an old UI command to the first
    // current track after project reload.
    const auto tracks = get_tracks_snapshot();
    const bool targetExists = std::any_of(
        tracks.begin(), tracks.end(), [tid](const auto& track) {
            return track != nullptr && track->getId() == tid;
        });
    if (!targetExists) {
        setError(CommandPushError::InvalidTarget);
        m_forensicJournaler.log(
            static_cast<uint32_t>(type), tid, val, ts);
        return false;
    }
    const uint64_t projectGeneration = get_project_generation();
    if (projectGeneration != expectedProjectGeneration) {
        setError(CommandPushError::StaleProjectGeneration);
        m_forensicJournaler.log(
            static_cast<uint32_t>(type), tid, val, ts);
        return false;
    }
    if (get_audio_config_generation() != expectedAudioGeneration) {
        setError(CommandPushError::StaleAudioGeneration);
        m_forensicJournaler.log(
            static_cast<uint32_t>(type), tid, val, ts);
        return false;
    }
    // LOCK-FREE Command Injection. State mutation is deferred to the audio
    // block consumer so the UI thread never races Track parameters.
    Command cmd{type, tid, val, ts, expectedAudioGeneration};
    // A full queue is a real command failure, not a best-effort enqueue. Do
    // not report success to the UI/CLI when the audio thread cannot observe
    // the command; otherwise the caller's undo/generation state diverges
    // from the engine.
    if (!m_commandQueue.push(cmd)) {
        setError(CommandPushError::QueueFull);
        m_forensicJournaler.log(
            static_cast<uint32_t>(type), tid, val, ts);
        ::Hirari::Core::Diagnostics::LogBuffer::post(
            1, tid, "COMMAND_QUEUE_FULL");
        return false;
    }
    setError(CommandPushError::None);

    // INDUSTRIAL: Real-time Journaling
    m_forensicJournaler.log(static_cast<uint32_t>(type), tid, val, ts);
    return true;
}

Track& HirariUnifiedEngine::get_track(uint32_t id) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& t : m_tracks) {
        if (t->getId() == id) return *t;
    }
    // Never redirect an unknown/stale identifier to the first live track.
    // Callers that need mutation must validate the ID before reaching this
    // compatibility reference; returning a real track here would silently
    // edit the wrong project object after reload.
    static Track fallbackTrack{0, "Fallback Track", Track::Audio};
    return fallbackTrack;
}

uint32_t HirariUnifiedEngine::get_track_count() const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    return static_cast<uint32_t>(m_tracks.size());
}

std::vector<std::shared_ptr<Track>> HirariUnifiedEngine::get_tracks_snapshot() const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    return m_tracks;
}

bool HirariUnifiedEngine::freeze_track(uint32_t tid, uint64_t totalSamples, uint32_t sampleRate) {
    if (totalSamples == 0 || sampleRate == 0 ||
        totalSamples > TrackFreezeManager::kMaxFreezeSamples) return false;
    // A freeze render mutates processor state while producing its immutable
    // snapshot, so it must never overlap the live callback or another graph
    // hydration.  The manager itself remains allocation/control-thread only.
    waitForAudioCallbacks();
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        const auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
            [tid](const auto& track) { return track && track->getId() == tid; });
        if (it == m_tracks.end()) return false;
        target = *it;
    }
    return TrackFreezeManager::freezeTrack(std::move(target), totalSamples, sampleRate);
}

bool HirariUnifiedEngine::freeze_track_to_project_end(uint32_t tid, uint32_t sampleRate) {
    if (sampleRate == 0) return false;
    std::shared_ptr<Track> target;
    uint64_t projectEnd = 0;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        const auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
            [tid](const auto& track) { return track && track->getId() == tid; });
        if (it == m_tracks.end()) return false;
        target = *it;
        for (const auto& region : target->getRegions()) {
            if (region.len == 0 || region.start > UINT64_MAX - region.len) return false;
            projectEnd = std::max(projectEnd, region.start + region.len);
        }
    }
    const uint64_t fallback = std::max<uint32_t>(1u, get_block_size()) * 2ull;
    const uint64_t totalSamples = std::max(projectEnd, fallback);
    if (totalSamples > TrackFreezeManager::kMaxFreezeSamples) return false;
    return freeze_track(tid, totalSamples, sampleRate);
}

bool HirariUnifiedEngine::freeze_track_to_file(uint32_t tid, std::string_view path,
                                             uint64_t totalSamples, uint32_t sampleRate) {
    if (path.empty() || totalSamples == 0 || sampleRate == 0 ||
        totalSamples > TrackFreezeManager::kMaxFreezeSamples) return false;
    waitForAudioCallbacks();
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        const auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
            [tid](const auto& track) { return track && track->getId() == tid; });
        if (it == m_tracks.end()) return false;
        target = *it;
    }
    std::error_code pathError;
    const auto absolutePath = std::filesystem::absolute(std::filesystem::path(path), pathError);
    if (pathError || absolutePath.empty()) return false;
    return TrackFreezeManager::freezeTrackToFile(
        std::move(target), absolutePath, totalSamples, sampleRate);
}

bool HirariUnifiedEngine::set_track_freeze_cache_path(uint32_t tid, std::string_view path) {
    if (path.empty()) return false;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    const auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
        [tid](const auto& track) { return track && track->getId() == tid; });
    return it != m_tracks.end() && (*it)->setFrozenCachePath(path);
}

bool HirariUnifiedEngine::restore_track_freeze_from_file(uint32_t tid, std::string_view path,
                                                       uint64_t totalSamples, uint32_t sampleRate) {
    if (path.empty() || totalSamples == 0 || sampleRate == 0) return false;
    waitForAudioCallbacks();
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        const auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
            [tid](const auto& track) { return track && track->getId() == tid; });
        if (it == m_tracks.end()) return false;
        target = *it;
    }
    return TrackFreezeManager::restoreTrackFromFile(
        std::move(target), std::filesystem::path(path), totalSamples, sampleRate);
}

bool HirariUnifiedEngine::unfreeze_track(uint32_t tid) {
    waitForAudioCallbacks();
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        const auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
            [tid](const auto& track) { return track && track->getId() == tid; });
        if (it == m_tracks.end()) return false;
        target = *it;
    }
    TrackFreezeManager::unfreezeTrack(std::move(target));
    return true;
}

bool HirariUnifiedEngine::is_track_frozen(uint32_t tid) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    const auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
        [tid](const auto& track) { return track && track->getId() == tid; });
    return it != m_tracks.end() && (*it)->isFrozen();
}

rust::Slice<const float> HirariUnifiedEngine::get_track_peaks_l() const {
    thread_local std::array<float, 256> snapshot{};
    TelemetryData telemetry{};
    if (!copy_telemetry(get_active_telemetry_idx(), telemetry)) {
        return rust::Slice<const float>(snapshot.data(), 0);
    }
    const uint32_t count = std::min<uint32_t>(telemetry.count, snapshot.size());
    std::copy_n(telemetry.peaksL, count, snapshot.data());
    return rust::Slice<const float>(snapshot.data(), count);
}

rust::Slice<const float> HirariUnifiedEngine::get_track_peaks_r() const {
    thread_local std::array<float, 256> snapshot{};
    TelemetryData telemetry{};
    if (!copy_telemetry(get_active_telemetry_idx(), telemetry)) {
        return rust::Slice<const float>(snapshot.data(), 0);
    }
    const uint32_t count = std::min<uint32_t>(telemetry.count, snapshot.size());
    std::copy_n(telemetry.peaksR, count, snapshot.data());
    return rust::Slice<const float>(snapshot.data(), count);
}

HirariUnifiedEngine::MeterData HirariUnifiedEngine::getMasterMeterData() const {
    if (m_analyzer) {
        auto d = m_analyzer->getLatestData();
        MeterData out{};
        out.peakL = d.peakL;
        out.peakR = d.peakR;
        out.truePeakL = d.truePeakL;
        out.truePeakR = d.truePeakR;
        out.rmsL = d.rmsL;
        out.rmsR = d.rmsR;
        out.lufsShortTerm = d.lufsShortTerm;
        out.lufsIntegrated = d.lufsIntegrated;
        out.correlation = d.correlation;
        out.balance = d.balance;
        return out;
    }
    return {};
}

double HirariUnifiedEngine::samples_to_beats(uint64_t samples) const {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    return hirari_tempo_map_samples_to_beats(m_tempoState, samples, sampleRate);
}

uint64_t HirariUnifiedEngine::beats_to_samples(double beats) const {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    return hirari_tempo_map_beats_to_samples(m_tempoState, beats, sampleRate);
}

