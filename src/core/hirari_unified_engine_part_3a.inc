

// Shared answer for a query whose track or slot could not be resolved; the
// Rust query surface names the fallback.
namespace {
constexpr size_t kSandboxQueryFailed = std::numeric_limits<size_t>::max();

size_t copySandboxText(const char* value, uint8_t* out, size_t capacity) {
    const std::string text(value ? value : "");
    if (out && capacity >= text.size()) std::memcpy(out, text.data(), text.size());
    return text.size();
}
} // namespace

// The maintenance pass itself lives in Rust; these handlers only resolve a
// track, report its sandbox slots and statuses, run one isolated block, and
// post the resulting event with the engine's playhead.
static_assert(sizeof(Track::SandboxStatus) == 24,
              "plugin_sandbox_maintenance.rs mirrors this layout");
void HirariUnifiedEngine::installSandboxMaintenanceHost() {
    static const HirariPluginSandboxHostOps ops = {
        [](void* host, void* context, HirariPluginSandboxVisitorFn visit) -> bool {
            if (!host) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            const std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (Track* track : engine->lockedTracks())
                for (const auto& status : track->getSandboxStatuses())
                    if (!visit(context, track->getId(), &status)) return true;
            return true;
        },
        [](void* host, uint32_t trackId, bool autoRestart, void* context,
           HirariPluginSandboxVisitorFn visit) -> uint32_t {
            if (!host) return 0;
            // The restart can block on a worker process, so the Track is held
            // by handle and the topology lock is already released around it.
            const auto track = static_cast<HirariUnifiedEngine*>(host)->takeTrackHandle(trackId);
            if (!track) return 0;
            const uint32_t recovered = track->maintainSandboxedPlugins(autoRestart);
            for (const auto& status : track->getSandboxStatuses())
                if (!visit(context, trackId, &status)) break;
            return recovered;
        },
        [](void* host, uint32_t type, uint32_t trackId, float value,
           const uint8_t* label, size_t labelLength) {
            if (!host) return;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            (void)hirari_engine_event_ring_push(
                engine->m_engineRings,
                hirari_engine_block_playhead(engine->m_engineBlock), type, trackId, value,
                label, labelLength);
        },
        [](void* host, void* context, HirariPluginSandboxSlotVisitorFn visit) -> bool {
            if (!host) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            const std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (Track* track : engine->lockedTracks())
                for (size_t slot = 0; slot < track->sandboxSlotCount(); ++slot) {
                    const auto path = track->sandboxSlotPath(slot);
                    if (!visit(context, track->getId(), static_cast<uint32_t>(slot),
                               track->sandboxSlotChainIndex(slot),
                               reinterpret_cast<const uint8_t*>(path.data()), path.size()))
                        return false;
                }
            return true;
        },
        [](void* host, uint32_t trackId, uint32_t slot, float* left, float* right,
           uint32_t frames) -> bool {
            if (!host) return false;
            return static_cast<HirariUnifiedEngine*>(host)->mutateLockedTrack(
                trackId, [&](Track& track) {
                    return track.processSandboxedPluginBlock(slot, left, right, frames);
                });
        },
        [](void* host, uint32_t trackId, uint32_t slot, uint32_t frames,
           const uint8_t* midiData, size_t midiLength, uint8_t* out,
           size_t capacity) -> size_t {
            if (!host) return kSandboxQueryFailed;
            std::vector<uint8_t> result;
            (void)static_cast<HirariUnifiedEngine*>(host)->mutateLockedTrack(
                trackId, [&](Track& track) {
                    result = track.processSandboxedPluginMidiBlock(
                        slot, frames, midiData, static_cast<uint32_t>(midiLength));
                    return true;
                });
            if (result.size() > capacity) return kSandboxQueryFailed;
            if (!result.empty()) std::memcpy(out, result.data(), result.size());
            return result.size();
        },
        [](void* host, uint32_t trackId, uint8_t* code, uint8_t* text,
           size_t capacity) -> size_t {
            if (!host) return kSandboxQueryFailed;
            return static_cast<HirariUnifiedEngine*>(host)->queryLockedTrack(
                trackId, kSandboxQueryFailed, [&](const Track& track) -> size_t {
                    if (code) *code = track.lastSandboxFailure();
                    return copySandboxText(track.lastSandboxFailureText(), text, capacity);
                });
        },
        [](void* host, uint32_t trackId, uint32_t slot, uint8_t* code, uint8_t* text,
           size_t capacity) -> size_t {
            if (!host) return kSandboxQueryFailed;
            return static_cast<HirariUnifiedEngine*>(host)->queryLockedTrack(
                trackId, kSandboxQueryFailed, [&](const Track& track) -> size_t {
                    if (code) *code = track.sandboxedPluginStateError(slot);
                    return copySandboxText(track.sandboxedPluginStateErrorText(slot),
                                           text, capacity);
                });
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
    const std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (Track* track : lockedTracks()) trips += track->takeWatchdogTrips();
    return trips;
}

uint64_t HirariUnifiedEngine::non_finite_plugin_samples() const {
    uint64_t count = 0;
    const std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (Track* track : lockedTracks()) count += track->nonFiniteSampleCount();
    return count;
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
// Native plugin editors require a processor that publishes a vendor UI. The
// chain's built-in and sandbox nodes never did, so these answers are constant
// and the whole API is a candidate for removal.
bool HirariUnifiedEngine::has_plugin_native_editor(uint32_t, uint32_t) const { return false; }
uint64_t HirariUnifiedEngine::open_plugin_native_editor(uint32_t, uint32_t, uintptr_t) const {
    return 0;
}
bool HirariUnifiedEngine::close_plugin_native_editor(uint32_t, uint32_t, uint64_t) const {
    return false;
}
std::string HirariUnifiedEngine::get_plugin_parameter_name(uint32_t, uint32_t, uint32_t) const {
    return {};
}
bool HirariUnifiedEngine::set_plugin_bypass(uint32_t tid, uint32_t pluginIndex, bool bypassed) {
    if (!hirari_plugin_edit_set_bypass(this, tid, pluginIndex, bypassed)) return false;
    // Bypass changes alter the active latency path. Rebuild the published
    // routing/PDC state now that the edit host has released m_tracksMutex.
    syncStructuralChanges();
    return true;
}
