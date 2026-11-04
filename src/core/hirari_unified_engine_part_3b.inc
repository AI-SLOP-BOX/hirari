bool HirariUnifiedEngine::get_plugin_bypass(uint32_t tid, uint32_t pluginIndex) const {
    return queryLockedTrack(tid, false,
                           [&](const Track& track) { return track.getPluginBypass(pluginIndex); });
}
uint32_t HirariUnifiedEngine::get_track_delay_samples(uint32_t tid) const {
    return queryLockedTrack(tid, uint32_t{0},
                            [&](const Track& track) { return track.getTrackDelaySamples(); });
}

// Routing edit host callbacks. Rust owns the routing graph and the sidechain
// route table — both are Rust stores the engine only holds — so it reads and
// mutates them directly. These handlers remain only where C++ state is
// involved: the live Track objects and the lock that guards them.
void HirariUnifiedEngine::installRoutingEditHost() {
    static const HirariRoutingHostOps ops = {
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_routingEngine; },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_sidechainRoutes; },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_sidechainEngine; },
        [](void* host, uint32_t trackId, HirariRoutingTrackState* out) {
            if (!out) return false;
            return withHostTrack(host, trackId, false, [&](Track& track) {
                out->armed = track.isRecordArmed();
                out->phaseInverted = track.isPhaseInverted();
                out->delaySamples = track.getTrackDelaySamples();
                out->isBus = dynamic_cast<const BusTrack*>(&track) != nullptr;
                out->volume = track.getVolume();
                out->pan = track.getPan();
                out->muted = track.isMuted();
                out->solo = track.isSolo();
                return true;
            });
        },
        [](void* host, uint32_t sourceId, uint32_t destId,
           HirariRoutingSidechainContext* out) {
            if (!out) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            // Rust's sidechain storage is 4096 frames and rejects longer links,
            // so the request is clamped before the source buffer is read.
            out->blockSize = std::min(engine->m_blockSize.load(std::memory_order_relaxed), 4096u);
            out->sampleRate = static_cast<uint32_t>(std::max(
                1.0, engine->m_sampleRate.load(std::memory_order_acquire)));
            out->generation = std::max<uint64_t>(
                1, hirari_engine_block_generation(engine->m_engineBlock));
            Track* source = engine->findTrackLocked(sourceId);
            Track* destination = engine->findTrackLocked(destId);
            out->sourceFound = source != nullptr;
            out->destinationFound = destination != nullptr;
            out->sourcePrepared = source && destination && source->canProcess(out->blockSize);
            return true;
        },
        [](void* host, uint32_t trackId, int32_t op, bool enabled, uint32_t samples) {
            return withHostTrack(host, trackId, false, [&](Track& track) {
                if (op == kHirariRoutingEditArm) track.setRecordArmed(enabled);
                else if (op == kHirariRoutingEditPhase) track.setPhaseInverted(enabled);
                else if (op == kHirariRoutingEditMute) track.setMuted(enabled);
                else if (op == kHirariRoutingEditSolo) track.setSolo(enabled);
                else track.setTrackDelaySamples(samples);
                return true;
            });
        },
        [](void* host, uint32_t destId, uint32_t pluginIndex, uint32_t sourceId,
           const HirariRoutingSidechainContext* context, uint32_t tapPoint, bool silent) {
            if (!context) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            if (silent) {
                return hirari_sidechain_engine_register_silent_link(
                    engine->m_sidechainEngine, destId, pluginIndex, sourceId,
                    context->blockSize, context->sampleRate, context->generation, 1.0f,
                    static_cast<uint8_t>(tapPoint));
            }
            Track* source = engine->findTrackLocked(sourceId);
            if (!source) return false;
            void* sourceBuffer = source->workBufferStorage();
            const uint32_t channels = hirari_audio_buffer_storage_channels(sourceBuffer);
            const float* left = hirari_audio_buffer_storage_read_pointer(sourceBuffer, 0, 0);
            const float* right = channels > 1
                ? hirari_audio_buffer_storage_read_pointer(sourceBuffer, 1, 0) : left;
            // Registration copies one prepared block into Rust-owned storage;
            // supplying the frame count is part of that contract.
            return hirari_sidechain_engine_register_link(
                engine->m_sidechainEngine, destId, pluginIndex, sourceId, left, right,
                context->blockSize, context->sampleRate, context->generation, 1.0f,
                static_cast<uint8_t>(tapPoint));
        },
        [](void* host) {
            static_cast<HirariUnifiedEngine*>(host)->mark_project_layout_changed();
        },
        [](void* host) {
            // syncStructuralChanges takes m_tracksMutex itself; Rust calls this
            // only after every mutation callback has released its lock.
            static_cast<HirariUnifiedEngine*>(host)->syncStructuralChanges();
        },
        nullptr,  // undo capture lives in the Rust routing edit host.
        [](void* host, uint32_t trackId, int32_t op, float value) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            Track* track = engine->findTrackLocked(trackId);
            if (!track) return false;
            if (op == kHirariRoutingEditVolume) track->setVolume(value);
            else if (op == kHirariRoutingEditPan) track->setPan(value);
            else return false;
            return true;
        },
        [](void* host) -> void* {
            return host ? static_cast<HirariUnifiedEngine*>(host)->undo_state() : nullptr;
        },
    };
    hirari_routing_edit_host_install(&ops);
}

// Control edit host callbacks. Rust validates every naming/monitor/macro edit
// and decides the undo entry; these handlers only read or apply the mutation.
void HirariUnifiedEngine::installControlEditHost() {
    static const HirariControlEditHostOps ops = {
        [](void* host, uint32_t trackId, uint8_t* out, size_t capacity) -> intptr_t {
            return withHostTrack(host, trackId, intptr_t{-1}, [&](Track& track) {
                const std::string& name = track.getName();
                if (!out || capacity == 0) return static_cast<intptr_t>(name.size());
                const size_t written = std::min(capacity, name.size());
                std::memcpy(out, name.data(), written);
                return static_cast<intptr_t>(written);
            });
        },
        [](void* host, uint32_t trackId, const uint8_t* name, size_t nameLen) {
            if (!name && nameLen > 0) return false;
            const std::string value =
                nameLen > 0 ? std::string(reinterpret_cast<const char*>(name), nameLen)
                            : std::string();
            return withHostTrack(host, trackId, false, [&](Track& track) {
                track.setName(value);
                return true;
            });
        },
        nullptr,  // undo capture lives in the Rust control edit host.
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
