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

bool HirariUnifiedEngine::collectProjectLayout(ProjectLayoutData* out, bool includeSaveOnlyFields,
                                               const std::atomic<bool>* cancellation) const {
    static_assert(sizeof(Hirari::Core::ProjectAutomationPoint) ==
                  sizeof(HirariLayoutAutomationPoint));

    const auto cancelled = [cancellation]() noexcept {
        return cancellation && cancellation->load(std::memory_order_acquire);
    };
    if (!out || cancelled()) return false;

    size_t feedbackCount = 0;
    void* rawFeedback = hirari_routing_engine_copy_feedback(m_routingEngine, &feedbackCount);
    std::vector<Hirari::Core::Engine::RoutingEngine::FeedbackConnection> feedbackConnections =
        takeRoutingFeedback(rawFeedback, feedbackCount);
    const double timelineSampleRate = m_sampleRate.load(std::memory_order_relaxed);

    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (!track) continue;
        // A callback may have queued write-automation points since the
        // previous snapshot. Drain them on this control-side boundary so UI,
        // CLI generation checks, history diffs, and project saves all observe
        // the same automation state.
        track->flushRecordedAutomation();
        track->reclaimEffectChainRetiredMemory();
    }

    const auto layoutString = [](const std::string& value) {
        HirariLayoutString result;
        result.data = reinterpret_cast<const uint8_t*>(value.data());
        result.len = value.size();
        return result;
    };
    const auto layoutPoints = [](const std::vector<Hirari::Core::ProjectAutomationPoint>& input) {
        std::vector<HirariLayoutAutomationPoint> points(input.size());
        for (size_t index = 0; index < input.size(); ++index) {
            points[index] = {input[index].time, input[index].value, input[index].curve};
        }
        return points;
    };

    out->storages.reserve(m_tracks.size());
    out->trackViews.reserve(m_tracks.size());
    for (const auto& track : m_tracks) {
        if (cancelled()) return false;
        if (!track) continue;
        auto storage = std::make_unique<ProjectLayoutStorage>();
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

        storage->pluginTypes = track->getPluginTypes();
        const auto& pluginTypes = storage->pluginTypes;
        storage->pluginBypass.resize(pluginTypes.size());
        for (size_t index = 0; index < pluginTypes.size(); ++index) {
            storage->pluginBypass[index] =
                track->getPluginBypass(static_cast<uint32_t>(index)) ? 1 : 0;
        }
        storage->pluginStates = track->getPluginStates();
        storage->pluginStateBlocks.resize(storage->pluginStates.size());
        for (size_t index = 0; index < storage->pluginStates.size(); ++index) {
            storage->pluginStateBlocks[index] = {
                storage->pluginStates[index].data(), storage->pluginStates[index].size()};
        }
        if (includeSaveOnlyFields) {
            storage->pluginGuiStates = track->getPluginGuiStates();
            storage->pluginGuiStateBlocks.resize(storage->pluginGuiStates.size());
            for (size_t index = 0; index < storage->pluginGuiStates.size(); ++index) {
                storage->pluginGuiStateBlocks[index] = {
                    storage->pluginGuiStates[index].data(),
                    storage->pluginGuiStates[index].size()};
            }
        } else {
            storage->pluginParameters.reserve(pluginTypes.size());
            for (size_t index = 0; index < pluginTypes.size(); ++index) {
                if (cancelled()) return false;
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
            if (cancelled()) return false;
            const auto& region = regions[index];
            auto& regionView = storage->regions[index];
            regionView.id = region.id;
            regionView.name = layoutString(region.name);
            regionView.state = region.editableState();
            regionView.sourceSampleRate = static_cast<uint32_t>(std::clamp(
                std::llround(region.sourceSampleRate > 0.0
                    ? region.sourceSampleRate : timelineSampleRate), 8000LL, 384000LL));
            regionView.path = layoutString(region.path);
            std::error_code pathError;
            regionView.missing = region.path.empty() ||
                !std::filesystem::exists(std::filesystem::path(region.path), pathError);
            // Distinguish a missing file from a file that exists but was not
            // decoded during project hydration; clients can surface a relink
            // or decoder error instead of silently showing an empty region.
            regionView.audioLoaded = region.audio != nullptr;
            regionView.warpMarkers = region.clip()->warpMarkers;
            regionView.warpMarkerCount = region.clip()->warpMarkerCount;
            regionView.rangeEdits = region.clip()->rangeEdits;
            regionView.rangeEditCount = region.clip()->rangeEditCount;
            regionView.noteStore = region.noteStore.get();
            regionView.noteFrozen = region.noteSnapshot.get();
            regionView.processingStore = region.processingStore.get();
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
        view.spatialX = track->getSpatialX();
        view.spatialY = track->getSpatialY();
        view.spatialZ = track->getSpatialZ();
        view.pluginGuiStates = storage->pluginGuiStateBlocks.data();
        view.pluginGuiStateCount = storage->pluginGuiStateBlocks.size();

        out->trackViews.push_back(view);
        out->storages.push_back(std::move(storage));
    }

    static_assert(sizeof(HirariLayoutSidechainRoute) == sizeof(HirariSidechainRoute));
    out->sidechainRoutes.resize(hirari_sidechain_routes_count(m_sidechainRoutes));
    (void)hirari_sidechain_routes_copy(
        m_sidechainRoutes,
        reinterpret_cast<HirariSidechainRoute*>(out->sidechainRoutes.data()),
        out->sidechainRoutes.size());
    out->feedbackRoutes.reserve(feedbackConnections.size());
    for (const auto& route : feedbackConnections) {
        out->feedbackRoutes.push_back({route.sourceId, route.destId, route.gain});
    }

    auto& snapshot = out->snapshot;
    snapshot.tracks = out->trackViews.data();
    snapshot.trackCount = out->trackViews.size();
    snapshot.sidechainRoutes = out->sidechainRoutes.data();
    snapshot.sidechainRouteCount = out->sidechainRoutes.size();
    snapshot.feedbackRoutes = out->feedbackRoutes.data();
    snapshot.feedbackRouteCount = out->feedbackRoutes.size();
    snapshot.timelineSampleRate = timelineSampleRate;
    return true;
}

rust::String HirariUnifiedEngine::get_project_layout_json_cancellable(
    const std::atomic<bool>* cancellation) const {
    // The serializer is Rust-owned. This function only gathers the current
    // model into a borrowed snapshot and keeps it alive (and stable) for the
    // synchronous call. Every ordering, escaping and formatting rule lives in
    // hirari-core-bridge/src/project_layout.rs.
    if (cancellation && cancellation->load(std::memory_order_acquire)) return rust::String();
    ProjectLayoutData data;
    if (!collectProjectLayout(&data, false, cancellation)) return rust::String();

    const auto isCancelled = [](const void* context) -> bool {
        const auto* flag = static_cast<const std::atomic<bool>*>(context);
        return flag && flag->load(std::memory_order_acquire);
    };
    const uint8_t* jsonData = nullptr;
    size_t jsonSize = 0;
    if (!hirari_project_layout_write_json(&data.snapshot, cancellation ? isCancelled : nullptr,
                                          cancellation, &jsonData, &jsonSize)) {
        return rust::String();
    }
    rust::String result(jsonData ? reinterpret_cast<const char*>(jsonData) : "", jsonSize);
    hirari_project_layout_json_free(jsonData, jsonSize);
    return result;
}

rust::String HirariUnifiedEngine::get_routing_snapshot_json() const {
    char* json = hirari_routing_engine_connections_json(m_routingEngine);
    if (!json) return rust::String("[]");
    rust::String result(json);
    hirari_routing_engine_connections_json_free(json);
    return result;
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

void HirariUnifiedEngine::installCommandHost() {
    static const HirariCommandHostOps ops = {
        [](void* host, uint32_t tid) -> bool {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            if (!engine) return false;
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            return engine->findTrackLocked(tid) != nullptr;
        },
        [](void* host, uint64_t* project, uint64_t* audio) -> bool {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            if (!engine) return false;
            *project = engine->get_project_generation();
            *audio = engine->get_audio_config_generation();
            return true;
        },
        [](void* host, uint32_t type, uint32_t tid, float val, uint64_t ts,
           uint64_t audioGeneration) -> bool {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            if (!engine) return false;
            const HirariEngineCommandRecord record{
                static_cast<uint32_t>(type), tid, val, ts, audioGeneration};
            return hirari_engine_command_ring_push(engine->m_engineRings, &record);
        },
        [](void* host, uint32_t type, uint32_t tid, float val, uint64_t ts) {
            if (auto* engine = static_cast<HirariUnifiedEngine*>(host))
                hirari_forensic_journaler_log(engine->m_forensicJournal, type, tid, val, ts);
        },
        [](void*, uint32_t tid) {
            hirari_log_post(1, tid, "COMMAND_QUEUE_FULL");
        },
    };
    hirari_command_host_install(&ops);
}

std::vector<std::shared_ptr<Track>> HirariUnifiedEngine::get_tracks_snapshot() const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    return m_tracks;
}

void HirariUnifiedEngine::installTrackFreezeHost() {
    static const HirariTrackFreezeHostOps ops = {
        [](void* host, uint32_t tid, bool wait) -> void* {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            if (!engine) return nullptr;
            if (wait) engine->waitForAudioCallbacks();
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            auto track = engine->findTrackSharedLocked(tid);
            return track ? new std::shared_ptr<Track>(std::move(track)) : nullptr;
        },
        [](void* handle) { delete static_cast<std::shared_ptr<Track>*>(handle); },
        [](void* handle, uint64_t totalSamples, uint32_t sampleRate) {
            return handle && TrackFreezeManager::freezeTrack(
                *static_cast<std::shared_ptr<Track>*>(handle), totalSamples, sampleRate);
        },
        [](void* handle, const uint8_t* path, size_t length, uint64_t totalSamples,
           uint32_t sampleRate, bool persist) {
            if (!handle) return false;
            auto track = *static_cast<std::shared_ptr<Track>*>(handle);
            const std::filesystem::path file(
                std::string(reinterpret_cast<const char*>(path), length));
            return persist
                ? TrackFreezeManager::freezeTrackToFile(track, file, totalSamples, sampleRate)
                : TrackFreezeManager::restoreTrackFromFile(track, file, totalSamples, sampleRate);
        },
        [](void* handle, const uint8_t* path, size_t length) {
            return handle && (*static_cast<std::shared_ptr<Track>*>(handle))->setFrozenCachePath(
                std::string_view(reinterpret_cast<const char*>(path), length));
        },
        [](void* handle) {
            if (handle) TrackFreezeManager::unfreezeTrack(
                *static_cast<std::shared_ptr<Track>*>(handle));
        },
        [](void* handle) {
            return handle && (*static_cast<std::shared_ptr<Track>*>(handle))->isFrozen();
        },
        [](void* handle, uint64_t* out) -> bool {
            if (!handle || !out) return false;
            uint64_t projectEnd = 0;
            for (const auto& region : (*static_cast<std::shared_ptr<Track>*>(handle))->getRegions()) {
                if (region.len == 0 || region.start > UINT64_MAX - region.len) return false;
                projectEnd = std::max(projectEnd, region.start + region.len);
            }
            *out = projectEnd;
            return true;
        },
        [](void* host) -> uint32_t {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            return engine ? engine->get_block_size() : 0;
        },
    };
    hirari_track_freeze_host_install(&ops);
}

HirariUnifiedEngine::MeterData HirariUnifiedEngine::getMasterMeterData() const {
    if (!m_meterCore) return {};
    float stats[4] = {-70.0f, -70.0f, -70.0f, -100.0f};
    hirari_master_meter_analysis_stats(m_meterCore, stats);
    MeterData out{};
    out.peakL = hirari_master_meter_get(m_meterCore, 0);
    out.peakR = hirari_master_meter_get(m_meterCore, 1);
    out.truePeakL = hirari_master_meter_get(m_meterCore, 2);
    out.truePeakR = hirari_master_meter_get(m_meterCore, 3);
    out.rmsL = hirari_master_meter_get(m_meterCore, 4);
    out.rmsR = hirari_master_meter_get(m_meterCore, 5);
    out.lufsShortTerm = stats[0];
    out.lufsIntegrated = stats[2];
    return out;
}

double HirariUnifiedEngine::samples_to_beats(uint64_t samples) const {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    return hirari_tempo_map_samples_to_beats(m_tempoState, samples, sampleRate);
}

uint64_t HirariUnifiedEngine::beats_to_samples(double beats) const {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    return hirari_tempo_map_beats_to_samples(m_tempoState, beats, sampleRate);
}

