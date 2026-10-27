bool HirariUnifiedEngine::replace_region_audio(uint32_t tid, uint32_t rid, std::string_view path) {
    return hirari_region_edit_replace_audio(this, tid, rid,
                                            reinterpret_cast<const uint8_t*>(path.data()),
                                            path.size());
}
bool HirariUnifiedEngine::add_plugin(uint32_t tid, uint32_t pluginType) {
    return hirari_plugin_edit_add(this, tid, pluginType);
}

bool HirariUnifiedEngine::remove_plugin(uint32_t tid, uint32_t pluginIndex) {
    return hirari_plugin_edit_remove(this, tid, pluginIndex);
}

bool HirariUnifiedEngine::add_sandboxed_plugin(uint32_t tid, std::string_view pluginPath) {
    return hirari_plugin_edit_add_sandboxed(
        this, tid, reinterpret_cast<const uint8_t*>(pluginPath.data()), pluginPath.size());
}

bool HirariUnifiedEngine::process_sandboxed_plugin_block(uint32_t tid, uint32_t sandboxIndex,
                                                       float* left, float* right, uint32_t frames) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track && track->getId() == tid)
            return track->processSandboxedPluginBlock(sandboxIndex, left, right, frames);
    }
    return false;
}

std::vector<uint8_t> HirariUnifiedEngine::process_sandboxed_plugin_midi_block(
    uint32_t tid, uint32_t sandboxIndex, uint32_t frames,
    const uint8_t* midiData, uint32_t midiSize) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid)
            return track->processSandboxedPluginMidiBlock(sandboxIndex, frames, midiData, midiSize);
    }
    return {};
}

uint32_t HirariUnifiedEngine::maintain_sandboxed_plugins(bool autoRestart) {
    return hirari_plugin_sandbox_maintain(this, autoRestart);
}

// The maintenance pass itself lives in Rust; these handlers only resolve a
// track, report its sandbox statuses, restart what can be restarted, and post
// the resulting event with the engine's playhead.
static_assert(sizeof(Track::SandboxStatus) == 24,
              "plugin_sandbox_maintenance.rs mirrors this layout");
void HirariUnifiedEngine::installSandboxMaintenanceHost() {
    static const HirariPluginSandboxHostOps ops = {
        [](void* host, void* context, HirariPluginSandboxVisitorFn visit) -> bool {
            if (!host) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            for (const auto& track : engine->get_tracks_snapshot())
                for (const auto& status : track->getSandboxStatuses())
                    if (!visit(context, track->getId(), &status)) return true;
            return true;
        },
        [](void* host, uint32_t trackId, bool autoRestart, void* context,
           HirariPluginSandboxVisitorFn visit) -> uint32_t {
            if (!host) return 0;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            for (const auto& track : engine->get_tracks_snapshot()) {
                if (!track || track->getId() != trackId) continue;
                const uint32_t recovered = track->maintainSandboxedPlugins(autoRestart);
                for (const auto& status : track->getSandboxStatuses())
                    if (!visit(context, trackId, &status)) break;
                return recovered;
            }
            return 0;
        },
        [](void* host, uint32_t type, uint32_t trackId, float value,
           const uint8_t* label, size_t labelLength) {
            if (!host) return;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            ::Hirari::Core::EngineEvent event{};
            event.timestamp = engine->m_playhead.load(std::memory_order_relaxed);
            event.type = type;
            event.trackId = trackId;
            event.value = value;
            std::snprintf(event.label, sizeof(event.label), "%.*s",
                          static_cast<int>(labelLength),
                          reinterpret_cast<const char*>(label));
            engine->push_event(event);
        },
    };
    hirari_plugin_sandbox_maintenance_host_install(&ops);
}

namespace {
[[maybe_unused]] const bool kSandboxMaintenanceHostInstalled = [] {
    HirariUnifiedEngine::installSandboxMaintenanceHost();
    return true;
}();
}

bool HirariUnifiedEngine::retry_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == trackId)
            return track->retrySandboxedPlugin(sandboxIndex);
    }
    return false;
}

bool HirariUnifiedEngine::restart_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == trackId)
            return track->restartSandboxedPlugin(sandboxIndex);
    }
    return false;
}

uint32_t HirariUnifiedEngine::take_watchdog_trips() {
    uint32_t trips = 0;
    for (const auto& track : get_tracks_snapshot()) {
        if (track) trips += track->takeWatchdogTrips();
    }
    return trips;
}

uint64_t HirariUnifiedEngine::non_finite_plugin_samples() const {
    uint64_t count = 0;
    for (const auto& track : get_tracks_snapshot()) {
        if (track) count += track->nonFiniteSampleCount();
    }
    return count;
}

uint8_t HirariUnifiedEngine::get_last_sandbox_failure(uint32_t trackId) const {
    const auto tracks = get_tracks_snapshot();
    for (const auto& track : tracks) {
        if (track && track->getId() == trackId) return track->lastSandboxFailure();
    }
    return 0;
}

std::string HirariUnifiedEngine::get_last_sandbox_failure_text(uint32_t trackId) const {
    const auto tracks = get_tracks_snapshot();
    for (const auto& track : tracks) {
        if (track && track->getId() == trackId) return track->lastSandboxFailureText();
    }
    return "track-not-found";
}

std::vector<std::string> HirariUnifiedEngine::get_sandbox_plugin_paths() const {
    const auto tracks = get_tracks_snapshot();
    std::vector<std::string> result;
    for (const auto& track : tracks) {
        if (!track) continue;
        const auto& paths = track->getSandboxedPluginPaths();
        result.insert(result.end(), paths.begin(), paths.end());
    }
    return result;
}

std::vector<uint8_t> HirariUnifiedEngine::get_sandbox_plugin_state(
    uint32_t trackId, uint32_t sandboxIndex) const {
    const auto tracks = get_tracks_snapshot();
    for (const auto& track : tracks) {
        if (track && track->getId() == trackId)
            return track->getSandboxedPluginState(sandboxIndex);
    }
    return {};
}

bool HirariUnifiedEngine::set_sandbox_plugin_state(
    uint32_t trackId, uint32_t sandboxIndex, const std::vector<uint8_t>& state) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == trackId)
            return track->setSandboxedPluginState(sandboxIndex, state);
    }
    return false;
}

uint8_t HirariUnifiedEngine::get_sandbox_plugin_state_error(uint32_t trackId, uint32_t sandboxIndex) const {
    const auto tracks = get_tracks_snapshot();
    for (const auto& track : tracks)
        if (track && track->getId() == trackId) return track->sandboxedPluginStateError(sandboxIndex);
    return Hirari::Core::Plugins::SandboxProtocol::kStateErrorUnavailable;
}

std::string HirariUnifiedEngine::get_sandbox_plugin_state_error_text(uint32_t trackId, uint32_t sandboxIndex) const {
    const auto tracks = get_tracks_snapshot();
    for (const auto& track : tracks)
        if (track && track->getId() == trackId) return track->sandboxedPluginStateErrorText(sandboxIndex);
    return "state-unavailable";
}
bool HirariUnifiedEngine::set_plugin_parameter(uint32_t tid, uint32_t pluginIndex,
                                             uint32_t parameterId, float value) {
    return hirari_plugin_edit_set_parameter(this, tid, pluginIndex, parameterId, value);
}

bool HirariUnifiedEngine::set_plugin_parameter_without_undo(uint32_t tid, uint32_t pluginIndex,
                                                          uint32_t parameterId, float value) {
    return hirari_plugin_edit_set_parameter_without_undo(
        this, tid, pluginIndex, parameterId, value);
}
float HirariUnifiedEngine::get_plugin_parameter(uint32_t tid, uint32_t pluginIndex,
                                              uint32_t parameterId) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid)
            return track->getPluginParameter(pluginIndex, parameterId);
    }
    return 0.0f;
}
uint32_t HirariUnifiedEngine::get_plugin_parameter_count(uint32_t tid, uint32_t pluginIndex) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks)
        if (track && track->getId() == tid) return track->getPluginParameterCount(pluginIndex);
    return 0;
}
bool HirariUnifiedEngine::has_plugin_native_editor(uint32_t tid, uint32_t pluginIndex) const {
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks)
            if (track && track->getId() == tid) {
                target = track;
                break;
            }
    }
    // Plugin/editor calls can enter vendor UI code. Keep the Track alive but
    // release the project topology lock before crossing that boundary.
    return target && target->hasPluginNativeEditor(pluginIndex);
}
uint64_t HirariUnifiedEngine::open_plugin_native_editor(uint32_t tid, uint32_t pluginIndex,
                                                      uintptr_t parent) const {
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks)
            if (track && track->getId() == tid) {
                target = track;
                break;
            }
    }
    return target ? target->openPluginNativeEditor(pluginIndex, parent) : 0;
}
bool HirariUnifiedEngine::close_plugin_native_editor(uint32_t tid, uint32_t pluginIndex,
                                                   uint64_t session) const {
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks)
            if (track && track->getId() == tid) {
                target = track;
                break;
            }
    }
    return target && target->closePluginNativeEditor(pluginIndex, session);
}
std::string HirariUnifiedEngine::get_plugin_parameter_name(uint32_t tid, uint32_t pluginIndex,
                                                         uint32_t parameterId) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks)
        if (track && track->getId() == tid)
            return track->getPluginParameterName(pluginIndex, parameterId);
    return {};
}
bool HirariUnifiedEngine::set_plugin_bypass(uint32_t tid, uint32_t pluginIndex, bool bypassed) {
    if (!hirari_plugin_edit_set_bypass(this, tid, pluginIndex, bypassed)) return false;
    // Bypass changes alter the active latency path. Rebuild the published
    // routing/PDC state now that the edit host has released m_tracksMutex.
    syncStructuralChanges();
    return true;
}
