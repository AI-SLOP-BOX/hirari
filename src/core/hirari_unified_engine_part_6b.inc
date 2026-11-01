float HirariUnifiedEngine::get_tempo() const {
    return hirari_tempo_map_get_current_bpm(m_tempoState);
}

bool HirariUnifiedEngine::set_tempo(float bpm) {
    // Thin entry point onto the Rust implementation, kept only because
    // tests/timeline_bpm_ghost_contract.cpp drives this method straight off
    // the engine singleton. Every other tempo edit reaches Rust without
    // entering C++ at all.
    return hirari_tempo_edit_set_tempo(
        m_tempoState, m_undoState,
        hirari_log_state(),
        m_playhead.load(std::memory_order_relaxed),
        m_sampleRate.load(std::memory_order_relaxed), bpm);
}

bool HirariUnifiedEngine::set_master_gain(float value) {
    // Storage only: the bridge owns the range check and the undo entry. The
    // audio callback reads this atomic once per block.
    m_masterGain.store(value, std::memory_order_release);
    return true;
}

float HirariUnifiedEngine::get_master_gain() const noexcept {
    return m_masterGain.load(std::memory_order_acquire);
}

size_t HirariUnifiedEngine::automationHostRead(void* host, uint32_t trackId, int32_t kind,
                                               uint32_t pluginIndex, uint32_t parameterId,
                                               Hirari::Core::ProjectAutomationPoint* out,
                                               size_t capacity, bool* targetFound) {
    if (targetFound) *targetFound = false;
    if (!host) return 0;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    const Track* track = engine->findTrackLocked(trackId);
    if (!track) return 0;
    std::vector<Hirari::Core::ProjectAutomationPoint> points;
    if (kind == kHirariAutomationPlugin) {
        if (pluginIndex >= track->getPluginTypes().size()) return 0;
        for (const auto& lane : track->getPluginAutomation()) {
            if (lane.pluginIndex == pluginIndex && lane.parameterId == parameterId) {
                points = lane.points;
                break;
            }
        }
    } else if (kind == kHirariAutomationTrackDelay) {
        points = track->getTrackDelayAutomationPoints();
    } else if (kind == kHirariAutomationPan) {
        points = track->getPanAutomationPoints();
    } else {
        points = track->getVolumeAutomationPoints();
    }
    if (targetFound) *targetFound = true;
    if (out && capacity >= points.size()) {
        std::copy(points.begin(), points.end(), out);
    }
    return points.size();
}

bool HirariUnifiedEngine::automationHostApply(void* host,
                                              const HirariAutomationEditPayload* payload) {
    if (!host || !payload) return false;
    if (payload->pointCount != 0 && payload->points == nullptr) return false;
    const std::vector<Hirari::Core::ProjectAutomationPoint> points(
        payload->points, payload->points + payload->pointCount);
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    Track* track = engine->findTrackLocked(payload->trackId);
    if (!track) return false;
    switch (payload->kind) {
        case kHirariAutomationVolume:
            track->setVolumeAutomation(points);
            return true;
        case kHirariAutomationPan:
            track->setPanAutomation(points);
            return true;
        case kHirariAutomationTrackDelay:
            track->setTrackDelayAutomation(points);
            return true;
        case kHirariAutomationPlugin:
            track->replacePluginAutomationLane(payload->pluginIndex,
                                               payload->parameterId, points);
            return true;
        default:
            return false;
    }
}

namespace {

const HirariAutomationHostOps kAutomationEditHostOps = {
    &HirariUnifiedEngine::automationHostRead,
    &HirariUnifiedEngine::automationHostApply,
    nullptr,  // undo capture lives in the Rust automation edit host.
    [](void* host) -> void* {
        return host ? static_cast<HirariUnifiedEngine*>(host)->undo_state() : nullptr;
    },
};

}  // namespace
void HirariUnifiedEngine::installTrackControlHost() {
    static const HirariTrackControlHostOps ops = {
        [](void* host, uint32_t tid) -> void* {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            if (!engine) return nullptr;
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            auto track = engine->findTrackSharedLocked(tid);
            return track ? new std::shared_ptr<Track>(std::move(track)) : nullptr;
        },
        [](void* handle) { delete static_cast<std::shared_ptr<Track>*>(handle); },
        [](void* handle, float x, float y, float z) {
            (*static_cast<std::shared_ptr<Track>*>(handle))->setSpatialPosition(x, y, z);
        },
        [](void* handle, const float* left, const float* right, uint32_t taps) -> bool {
            return (*static_cast<std::shared_ptr<Track>*>(handle))->setHrtfKernel(left, right, taps);
        },
        [](void* handle) {
            (*static_cast<std::shared_ptr<Track>*>(handle))->clearHrtfKernel();
        },
    };
    hirari_track_control_host_install(&ops);
}

bool HirariUnifiedEngine::execute_auto_arrangement() {
    // Project extent, section boundaries and marker lengths are Rust-owned.
    // The engine keeps the region storage and the event queue behind the
    // host callbacks and the marker sink below.
    float harmonyTension = 0.5f;
    float harmonyValence = 0.5f;
    hirari_harmonic_context_get(&harmonyTension, &harmonyValence, nullptr);
    return hirari_engine_auto_arrange(
        this, hirari_tempo_map_get_current_bpm(m_tempoState), m_sampleRate.load(),
        harmonyTension, harmonyValence,
        [](void* context, uint64_t start, uint64_t length) {
            auto* engine = static_cast<HirariUnifiedEngine*>(context);
            // EVENT_TYPE_STRUCTURE_MARKER carries the section length as its value.
            (void)hirari_engine_event_ring_push(engine->m_engineRings, start, 0x50, 0,
                                                static_cast<float>(length), nullptr, 0);
            hirari_log_post(0, 0, "AI | ARRANGEMENT | SECTION_DETECTED");
        });
}

namespace {
// Every host table is installed once, at image load and before any Rust caller
// can reach the engine. One owner replaces the per-table lazy initializers, so
// the install order is deterministic instead of depending on a linker's
// cross-file static order.
[[maybe_unused]] const bool kEngineHostTablesInstalled = [] {
    HirariUnifiedEngine::installTrackEditHost();
    HirariUnifiedEngine::installSandboxMaintenanceHost();
    HirariUnifiedEngine::installRoutingEditHost();
    HirariUnifiedEngine::installControlEditHost();
    HirariUnifiedEngine::installPluginEditHost();
    HirariUnifiedEngine::installExpressionMapEditHost();
    HirariUnifiedEngine::installTrackFreezeHost();
    HirariUnifiedEngine::installCommandHost();
    HirariUnifiedEngine::installTrackControlHost();
    hirari_region_host_install(&kRegionEditHostOps);
    hirari_automation_host_install(&kAutomationEditHostOps);
    return true;
}();
}  // namespace
