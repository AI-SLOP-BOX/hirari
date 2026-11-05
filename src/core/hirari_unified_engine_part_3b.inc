// Routing edit host callbacks. Rust owns the routing graph, the sidechain route
// table, the sidechain gate's registration inputs and every track sub-state a
// routing edit reads or writes — all of them are Rust stores the engine only
// holds — so it reads and mutates them through the shared track session and the
// engine block state. These handlers hand over those store addresses plus the
// two engine operations that must stay here: the layout mark and the graph
// resync.
void HirariUnifiedEngine::installRoutingEditHost() {
    static const HirariRoutingHostOps ops = {
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->handle(kHirariHandleRouting); },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->handle(kHirariHandleSidechainRoutes); },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->handle(kHirariHandleSidechainEngine); },
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
            return host ? static_cast<HirariUnifiedEngine*>(host)->handle(kHirariHandleEngineBlock) : nullptr;
        },
    };
    hirari_routing_edit_host_install(&ops);
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

bool HirariUnifiedEngine::has_sidechain_link(uint32_t sourceId, uint32_t destId,
                                           uint32_t pluginIndex) const {
    return hirari_sidechain_engine_has_link(
        handle(kHirariHandleSidechainEngine), destId, pluginIndex, sourceId);
}
