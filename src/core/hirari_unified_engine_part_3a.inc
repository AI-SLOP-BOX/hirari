

bool HirariUnifiedEngine::process_sandboxed_plugin_block(uint32_t tid, uint32_t sandboxIndex,
                                                       float* left, float* right, uint32_t frames) {
    return mutateLockedTrack(tid, [&](Track& track) {
        return track.processSandboxedPluginBlock(sandboxIndex, left, right, frames);
    });
}

std::vector<uint8_t> HirariUnifiedEngine::process_sandboxed_plugin_midi_block(
    uint32_t tid, uint32_t sandboxIndex, uint32_t frames,
    const uint8_t* midiData, uint32_t midiSize) {
    std::vector<uint8_t> result;
    (void)mutateLockedTrack(tid, [&](Track& track) {
        result = track.processSandboxedPluginMidiBlock(sandboxIndex, frames, midiData, midiSize);
        return true;
    });
    return result;
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
            (void)hirari_engine_event_ring_push(
                engine->m_engineRings,
                engine->m_playhead.load(std::memory_order_relaxed), type, trackId, value,
                label, labelLength);
        },
    };
    hirari_plugin_sandbox_maintenance_host_install(&ops);
}

bool HirariUnifiedEngine::retry_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex) {
    return mutateLockedTrack(trackId,
                             [&](Track& track) { return track.retrySandboxedPlugin(sandboxIndex); });
}

bool HirariUnifiedEngine::restart_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex) {
    return mutateLockedTrack(trackId,
                             [&](Track& track) { return track.restartSandboxedPlugin(sandboxIndex); });
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
    return querySnapshotTrack(trackId, uint8_t{0},
                              [](const Track& track) { return track.lastSandboxFailure(); });
}

// An unresolved track reports an empty text; naming the not-found fallback is
// the Rust query surface's job.
std::string HirariUnifiedEngine::get_last_sandbox_failure_text(uint32_t trackId) const {
    return querySnapshotTrack(trackId, std::string{},
                              [](const Track& track) -> std::string {
                                  return track.lastSandboxFailureText();
                              });
}

std::vector<std::string> HirariUnifiedEngine::get_sandbox_plugin_paths() const {
    std::vector<std::string> result;
    for (const auto& track : get_tracks_snapshot()) {
        if (!track) continue;
        const auto& paths = track->getSandboxedPluginPaths();
        result.insert(result.end(), paths.begin(), paths.end());
    }
    return result;
}

std::vector<uint8_t> HirariUnifiedEngine::get_sandbox_plugin_state(
    uint32_t trackId, uint32_t sandboxIndex) const {
    return querySnapshotTrack(trackId, std::vector<uint8_t>{},
                              [&](const Track& track) {
                                  return track.getSandboxedPluginState(sandboxIndex);
                              });
}

bool HirariUnifiedEngine::set_sandbox_plugin_state(
    uint32_t trackId, uint32_t sandboxIndex, const std::vector<uint8_t>& state) {
    return mutateLockedTrack(trackId, [&](Track& track) {
        return track.setSandboxedPluginState(sandboxIndex, state);
    });
}

uint8_t HirariUnifiedEngine::get_sandbox_plugin_state_error(uint32_t trackId, uint32_t sandboxIndex) const {
    return querySnapshotTrack(
        trackId, Hirari::Core::Plugins::SandboxProtocol::kStateErrorUnavailable,
        [&](const Track& track) { return track.sandboxedPluginStateError(sandboxIndex); });
}

std::string HirariUnifiedEngine::get_sandbox_plugin_state_error_text(uint32_t trackId, uint32_t sandboxIndex) const {
    return querySnapshotTrack(trackId, std::string{}, [&](const Track& track) -> std::string {
        return track.sandboxedPluginStateErrorText(sandboxIndex);
    });
}

float HirariUnifiedEngine::get_plugin_parameter(uint32_t tid, uint32_t pluginIndex,
                                              uint32_t parameterId) const {
    return queryLockedTrack(tid, 0.0f, [&](const Track& track) {
        return track.getPluginParameter(pluginIndex, parameterId);
    });
}
uint32_t HirariUnifiedEngine::get_plugin_parameter_count(uint32_t tid, uint32_t pluginIndex) const {
    return queryLockedTrack(tid, 0u,
                            [&](const Track& track) { return track.getPluginParameterCount(pluginIndex); });
}
bool HirariUnifiedEngine::has_plugin_native_editor(uint32_t tid, uint32_t pluginIndex) const {
    const auto target = takeTrackHandle(tid);
    // Plugin/editor calls can enter vendor UI code, so the Track stays alive
    // while the project topology lock is already released.
    return target && target->hasPluginNativeEditor(pluginIndex);
}
uint64_t HirariUnifiedEngine::open_plugin_native_editor(uint32_t tid, uint32_t pluginIndex,
                                                      uintptr_t parent) const {
    const auto target = takeTrackHandle(tid);
    return target ? target->openPluginNativeEditor(pluginIndex, parent) : 0;
}
bool HirariUnifiedEngine::close_plugin_native_editor(uint32_t tid, uint32_t pluginIndex,
                                                   uint64_t session) const {
    const auto target = takeTrackHandle(tid);
    return target && target->closePluginNativeEditor(pluginIndex, session);
}
std::string HirariUnifiedEngine::get_plugin_parameter_name(uint32_t tid, uint32_t pluginIndex,
                                                         uint32_t parameterId) const {
    return queryLockedTrack(tid, std::string{}, [&](const Track& track) -> std::string {
        return track.getPluginParameterName(pluginIndex, parameterId);
    });
}
bool HirariUnifiedEngine::set_plugin_bypass(uint32_t tid, uint32_t pluginIndex, bool bypassed) {
    if (!hirari_plugin_edit_set_bypass(this, tid, pluginIndex, bypassed)) return false;
    // Bypass changes alter the active latency path. Rebuild the published
    // routing/PDC state now that the edit host has released m_tracksMutex.
    syncStructuralChanges();
    return true;
}
