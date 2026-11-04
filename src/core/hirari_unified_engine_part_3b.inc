bool HirariUnifiedEngine::get_plugin_bypass(uint32_t tid, uint32_t pluginIndex) const {
    return queryLockedTrack(tid, false,
                           [&](const Track& track) { return track.getPluginBypass(pluginIndex); });
}
uint32_t HirariUnifiedEngine::get_track_delay_samples(uint32_t tid) const {
    return queryLockedTrack(tid, uint32_t{0},
                            [&](const Track& track) { return track.getTrackDelaySamples(); });
}

// Routing edit host callbacks. Rust owns the routing graph, the sidechain route
// table, the sidechain gate's registration inputs and every track sub-state a
// routing edit reads or writes — all of them are Rust stores the engine only
// holds — so it reads and mutates them through the shared track session and the
// engine block state. These handlers hand over those store addresses plus the
// two engine operations that must stay here: the layout mark and the graph
// resync.
void HirariUnifiedEngine::installRoutingEditHost() {
    static const HirariRoutingHostOps ops = {
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_routingEngine; },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_sidechainRoutes; },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_sidechainEngine; },
        nullptr,  // sidechain context and registration live in the Rust routing edit host.
        nullptr,
        [](void* host) {
            static_cast<HirariUnifiedEngine*>(host)->mark_project_layout_changed();
        },
        [](void* host) {
            // syncStructuralChanges takes m_tracksMutex itself; Rust calls this
            // only after every mutation callback has released its lock.
            static_cast<HirariUnifiedEngine*>(host)->syncStructuralChanges();
        },
        nullptr,  // undo capture lives in the Rust routing edit host.
        [](void* host) -> void* {
            return host ? static_cast<HirariUnifiedEngine*>(host)->undo_state() : nullptr;
        },
        [](void* host) -> void* {
            return host ? static_cast<HirariUnifiedEngine*>(host)->m_engineBlock : nullptr;
        },
    };
    hirari_routing_edit_host_install(&ops);
}

// Control edit host callbacks. Rust validates every monitor/macro edit and
// decides the undo entry; these handlers only read or apply the mutation. The
// track name lives in each track's own Rust store, so naming needs no adapter.
void HirariUnifiedEngine::installControlEditHost() {
    static const HirariControlEditHostOps ops = {
        [](void* host, uint32_t trackId, bool enabled, uint32_t leftChannel,
           uint32_t rightChannel) -> bool {
            if (!host) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            if (!enabled) {
                hirari_input_blocks_clear_focus(engine->m_inputBlocks, trackId);
                return true;
            }
            {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                if (!engine->findTrackLocked(trackId)) return false;
            }
            // One physical input is monitored at a time, so enabling another
            // track moves the focus without touching record-arm state.
            return hirari_input_blocks_set_focus(engine->m_inputBlocks, trackId, leftChannel,
                                                rightChannel);
        },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_macroControl; },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_midiLearnState; },
        [](void* host) -> void* {
            return host ? static_cast<HirariUnifiedEngine*>(host)->undo_state() : nullptr;
        },
    };
    hirari_control_edit_host_install(&ops);
}

// Plugin-chain edit host callbacks. Rust decides whether an edit is admitted,
// applies it to the track's own effect chain and records the undo entry; only
// the graph resync and the history handle are the engine's.
void HirariUnifiedEngine::installPluginEditHost() {
    static const HirariPluginEditHostOps ops = {
        &HirariUnifiedEngine::syncNative,
        [](void* host) -> void* {
            return host ? static_cast<HirariUnifiedEngine*>(host)->undo_state() : nullptr;
        },
    };
    hirari_plugin_edit_host_install(&ops);
}

// The one track-edit session every control-side host shares (track_session.rs).
// Rust holds the tracks lock for the edit, writes the stores the track owns and
// keeps the frozen-render/layout-revision protocol; these callbacks are the
// pieces that physically cannot leave C++: the lock, the live track table, the
// handles a Track holds, and re-preparing it for playback.
void HirariUnifiedEngine::installTrackSessionHost() {
    static const HirariTrackSessionOps ops = {
        [](void* host) { static_cast<HirariUnifiedEngine*>(host)->m_tracksMutex.lock(); },
        [](void* host) { static_cast<HirariUnifiedEngine*>(host)->m_tracksMutex.unlock(); },
        [](void* host, uint32_t trackId) -> void* {
            return static_cast<HirariUnifiedEngine*>(host)->findTrackLocked(trackId);
        },
        &HirariUnifiedEngine::trackStorageHandle,
        [](void* track) -> uint32_t {
            return track ? static_cast<uint32_t>(static_cast<Track*>(track)->getType()) : 0u;
        },
        [](void* track) {
            if (track) static_cast<Track*>(track)->markProjectLayoutChanged();
        },
        [](void* host, void* track) {
            if (!host || !track) return;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            static_cast<Track*>(track)->prepareToPlay(
                engine->m_sampleRate.load(std::memory_order_relaxed),
                engine->m_blockSize.load(std::memory_order_relaxed));
        },
    };
    hirari_track_session_install(&ops);
}


bool HirariUnifiedEngine::has_sidechain_link(uint32_t sourceId, uint32_t destId,
                                           uint32_t pluginIndex) const {
    return hirari_sidechain_engine_has_link(
        m_sidechainEngine, destId, pluginIndex, sourceId);
}
