

// The maintenance pass itself lives in Rust; these handlers only walk the graph
// and hand each track to the Rust sandbox queries, then post the resulting event
// with the engine's playhead. The status layout is plugin_sandbox_maintenance.rs.
void HirariUnifiedEngine::installSandboxMaintenanceHost() {
    static const HirariPluginSandboxHostOps ops = {
        [](void* host, void* context, HirariPluginSandboxVisitorFn visit) -> bool {
            if (!host) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            const std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (void* track : engine->lockedTracks())
                hirari_track_visit_sandbox_statuses(track, context, visit);
            return true;
        },
        [](void* host, uint32_t trackId, bool autoRestart, void* context,
           HirariPluginSandboxVisitorFn visit) -> uint32_t {
            if (!host) return 0;
            // The restart can block on a worker process, so the track is held by
            // handle and the topology lock is already released around it.
            void* owner = static_cast<HirariUnifiedEngine*>(host)->takeTrackOwner(trackId);
            if (!owner) return 0;
            const uint32_t recovered = hirari_track_maintain_sandboxes(
                hirari_track_owner_object(owner), autoRestart, context, visit);
            hirari_track_owner_release(owner);
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
// Parameter names and native editor sessions are Rust-side too: the chain host
// answers the name from its own effect tables and HirariCore holds the session
// registry, so this facade no longer forwards either.
bool HirariUnifiedEngine::set_plugin_bypass(uint32_t tid, uint32_t pluginIndex, bool bypassed) {
    if (!hirari_plugin_edit_set_bypass(this, tid, pluginIndex, bypassed)) return false;
    // Bypass changes alter the active latency path. Rebuild the published
    // routing/PDC state now that the edit host has released m_tracksMutex.
    syncStructuralChanges();
    return true;
}
