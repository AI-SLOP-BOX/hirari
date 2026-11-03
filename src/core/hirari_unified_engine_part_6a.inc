bool HirariUnifiedEngine::set_track_eq(uint32_t trackId, float lowBoostDb, float lowCutDb,
                                     float highBoostDb, float highCutDb) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    Track* track = findTrackLocked(trackId);
    if (!track) return false;
    track->setEq(lowBoostDb, lowCutDb, highBoostDb, highCutDb);
    return true;
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

namespace {
size_t layoutCopyString(const std::string& value, uint8_t* out, size_t capacity) {
    if (out && capacity > 0) std::memcpy(out, value.data(), std::min(capacity, value.size()));
    return value.size();
}
size_t layoutCopyBytes(const std::vector<uint8_t>& value, uint8_t* out, size_t capacity) {
    if (out && capacity > 0) std::memcpy(out, value.data(), std::min(capacity, value.size()));
    return value.size();
}
}  // namespace

// Exposes the live tracks to the Rust layout gather (layout_gather.rs). The
// per-track rules — owned string copies, plug-in ordering, region rate
// clamping, missing-file detection — all live there; these callbacks only read
// the model under the tracks lock that begin()/end() hold.
const HirariLayoutHostOps* HirariUnifiedEngine::layoutHostOps() const {
    static const HirariLayoutHostOps ops = {
        // Drain queued write-automation before taking the lock, so UI, CLI
        // generation checks, history diffs and saves observe one state.
        [](void* host) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            engine->m_tracksMutex.lock();
            for (Track* track : engine->lockedTracks()) {
                if (!track) continue;
                track->flushRecordedAutomation();
                track->reclaimEffectChainRetiredMemory();
            }
        },
        [](void* host) {
            static_cast<HirariUnifiedEngine*>(host)->m_tracksMutex.unlock();
        },
        [](void* host) {
            return static_cast<HirariUnifiedEngine*>(host)->lockedTracks().size();
        },
        [](void* host, size_t index, HirariLayoutTrackScalars* out) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            if (!track) return false;
            out->id = track->getId();
            out->type = static_cast<uint32_t>(track->getType());
            out->volume = track->getVolume();
            out->pan = track->getPan();
            out->muted = track->isMuted();
            out->solo = track->isSolo();
            out->recordArmed = track->isRecordArmed();
            out->phaseInverted = track->isPhaseInverted();
            out->trackDelaySamples = track->getTrackDelaySamples();
            out->frozen = track->isFrozen();
            out->frozenTotalSamples = track->frozenTotalSamples();
            out->frozenSampleRate = track->frozenSampleRate();
            out->spatialX = track->getSpatialX();
            out->spatialY = track->getSpatialY();
            out->spatialZ = track->getSpatialZ();
            return true;
        },
        [](void* host, size_t index, uint32_t kind, uint8_t* out, size_t capacity) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            if (!track) return size_t{0};
            switch (kind) {
                case 0: return layoutCopyString(track->getName(), out, capacity);
                case 1: return layoutCopyString(track->frozenPath(), out, capacity);
                case 2: return layoutCopyString(track->getExpressionMapJson(), out, capacity);
                default: return layoutCopyString(track->getExpressionMapProJson(), out, capacity);
            }
        },
        [](void* host, size_t index, uint32_t kind, HirariLayoutAutomationPoint* out,
           size_t capacity) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            if (!track) return size_t{0};
            const std::vector<::Hirari::Core::ProjectAutomationPoint> points =
                kind == 0 ? track->getVolumeAutomationPoints()
                : kind == 1 ? track->getPanAutomationPoints()
                            : track->getTrackDelayAutomationPoints();
            static_assert(sizeof(HirariLayoutAutomationPoint) ==
                          sizeof(::Hirari::Core::ProjectAutomationPoint));
            if (out && capacity > 0) {
                const size_t count = std::min(capacity, points.size());
                std::memcpy(out, points.data(), count * sizeof(HirariLayoutAutomationPoint));
            }
            return points.size();
        },
        [](void* host, size_t index) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            return track ? track->getPluginAutomation().size() : size_t{0};
        },
        [](void* host, size_t index, size_t lane, HirariLayoutPluginLaneHeader* out) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            if (!track) return false;
            const auto lanes = track->getPluginAutomation();
            if (lane >= lanes.size()) return false;
            out->pluginIndex = lanes[lane].pluginIndex;
            out->parameterId = lanes[lane].parameterId;
            out->pointCount = lanes[lane].points.size();
            return true;
        },
        [](void* host, size_t index, size_t lane, HirariLayoutAutomationPoint* out,
           size_t capacity) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            if (!track) return size_t{0};
            const auto lanes = track->getPluginAutomation();
            if (lane >= lanes.size()) return size_t{0};
            const auto& points = lanes[lane].points;
            static_assert(sizeof(HirariLayoutAutomationPoint) ==
                          sizeof(::Hirari::Core::ProjectAutomationPoint));
            if (out && capacity > 0) {
                std::memcpy(out, points.data(),
                            std::min(capacity, points.size()) * sizeof(HirariLayoutAutomationPoint));
            }
            return points.size();
        },
        [](void* host, size_t index) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            return track ? track->getPluginTypes().size() : size_t{0};
        },
        [](void* host, size_t index, size_t plugin) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            const auto types = track ? track->getPluginTypes() : std::vector<uint32_t>{};
            return plugin < types.size() ? types[plugin] : 0u;
        },
        [](void* host, size_t index, size_t plugin) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            return track && track->getPluginBypass(static_cast<uint32_t>(plugin));
        },
        [](void* host, size_t index, size_t plugin, uint8_t* out, size_t capacity) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            const auto states = track ? track->getPluginStates()
                                      : std::vector<std::vector<uint8_t>>{};
            if (plugin >= states.size()) return size_t{0};
            return layoutCopyBytes(states[plugin], out, capacity);
        },
        [](void* host, size_t index, size_t plugin, uint8_t* out, size_t capacity) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            const auto states = track ? track->getPluginGuiStates()
                                      : std::vector<std::vector<uint8_t>>{};
            if (plugin >= states.size()) return size_t{0};
            return layoutCopyBytes(states[plugin], out, capacity);
        },
        [](void* host, size_t index, size_t plugin) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            return track ? track->getPluginParameterCount(static_cast<uint32_t>(plugin))
                         : size_t{0};
        },
        [](void* host, size_t index, size_t plugin, float* out, size_t capacity) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            if (!track) return size_t{0};
            const uint32_t count =
                track->getPluginParameterCount(static_cast<uint32_t>(plugin));
            const size_t written = std::min<size_t>(capacity, count);
            for (size_t parameter = 0; parameter < written; ++parameter) {
                out[parameter] = track->getPluginParameter(
                    static_cast<uint32_t>(plugin), static_cast<uint32_t>(parameter));
            }
            return static_cast<size_t>(count);
        },
        [](void* host, size_t index) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            return track ? track->getSandboxedPluginPaths().size() : size_t{0};
        },
        [](void* host, size_t index, size_t slot, uint8_t* out, size_t capacity) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            const auto paths = track ? track->getSandboxedPluginPaths()
                                     : std::vector<std::string>{};
            return slot < paths.size() ? layoutCopyString(paths[slot], out, capacity) : size_t{0};
        },
        [](void* host, size_t index, size_t slot, uint8_t* out, size_t capacity) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            const auto states = track ? track->getSandboxedPluginStates()
                                      : std::vector<std::vector<uint8_t>>{};
            return slot < states.size() ? layoutCopyBytes(states[slot], out, capacity) : size_t{0};
        },
        [](void* host, size_t index) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            return track ? hirari_region_store_count(const_cast<Track*>(track)) : size_t{0};
        },
        [](void* host, size_t index, size_t regionIndex, HirariLayoutRegionHeader* out) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            return track && hirari_region_store_layout_header(const_cast<Track*>(track), regionIndex, out);
        },
        [](void* host, size_t index, size_t regionIndex, uint32_t kind, uint8_t* out,
           size_t capacity) {
            const Track* track = static_cast<const HirariUnifiedEngine*>(host)->lockedTrackAt(index);
            return track ? hirari_region_store_layout_string(const_cast<Track*>(track), regionIndex, kind,
                                                             out, capacity)
                         : size_t{0};
        },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_sidechainRoutes; },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_routingEngine; },
        [](void* host) {
            return static_cast<HirariUnifiedEngine*>(host)
                ->m_sampleRate.load(std::memory_order_relaxed);
        },
    };
    return &ops;
}

rust::String HirariUnifiedEngine::get_project_layout_json_cancellable(
    const std::atomic<bool>* cancellation) const {
    // The gather, snapshot and serializer are all Rust-owned; this only holds
    // the returned JSON buffer for the length of the copy below.
    if (cancellation && cancellation->load(std::memory_order_acquire)) return rust::String();
    const auto isCancelled = [](const void* context) -> bool {
        const auto* flag = static_cast<const std::atomic<bool>*>(context);
        return flag && flag->load(std::memory_order_acquire);
    };
    uint8_t* jsonData = nullptr;
    size_t jsonSize = 0;
    const bool written = hirari_layout_json_from_host(
        const_cast<HirariUnifiedEngine*>(this), layoutHostOps(),
        cancellation ? isCancelled : nullptr, cancellation, &jsonData, &jsonSize);
    rust::String result(written && jsonData ? reinterpret_cast<const char*>(jsonData) : "",
                        written ? jsonSize : 0);
    hirari_project_layout_json_free(jsonData, jsonSize);
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
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_engineRings; },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_forensicJournal; },
    };
    hirari_command_host_install(&ops);
}

void HirariUnifiedEngine::installTrackFreezeHost() {
    static const HirariTrackFreezeHostOps ops = {
        [](void* host, uint32_t tid, bool wait) -> void* {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            if (!engine) return nullptr;
            if (wait) engine->waitForAudioCallbacks();
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            return newTrackHandle(engine->findTrackSharedLocked(tid));
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
            const auto track = *static_cast<std::shared_ptr<Track>*>(handle);
            return track && hirari_region_store_project_end(track.get(), out);
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


