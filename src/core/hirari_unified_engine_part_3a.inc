

// The maintenance pass itself lives in Rust; these handlers only report the
// per-track sandbox statuses, run one restart and post the resulting event with
// the engine's playhead. The slot queries and isolated block processing read the
// effect chain through the Rust layout gather.
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
    };
    hirari_plugin_sandbox_maintenance_host_install(&ops);
}

// The plug-in chain reads, the sandbox restarts and the channel EQ went to
// Rust (engine_analysis_reads.rs): a session resolves the live track and the
// chain or runtime storage answers, so these engine methods no longer exist.
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
