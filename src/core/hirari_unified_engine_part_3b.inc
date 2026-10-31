bool HirariUnifiedEngine::get_plugin_bypass(uint32_t tid, uint32_t pluginIndex) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    const Track* track = findTrackLocked(tid);
    return track && track->getPluginBypass(pluginIndex);
}
bool HirariUnifiedEngine::move_plugin(uint32_t tid, uint32_t fromIndex, uint32_t toIndex) {
    if (!hirari_plugin_edit_move(this, tid, fromIndex, toIndex)) return false;
    syncStructuralChanges();
    return true;
}
uint32_t HirariUnifiedEngine::get_track_delay_samples(uint32_t tid) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    const Track* track = findTrackLocked(tid);
    return track ? track->getTrackDelaySamples() : 0;
}

bool HirariUnifiedEngine::set_track_input_monitor(uint32_t tid, bool enabled) {
    return set_track_input_monitor_channels(tid, enabled, 0, 1);
}

bool HirariUnifiedEngine::set_track_input_monitor_channels(
    uint32_t tid, bool enabled, uint32_t leftChannel, uint32_t rightChannel) {
    return hirari_control_edit_set_input_monitor(
        this, tid, enabled, leftChannel, rightChannel);
}

bool HirariUnifiedEngine::set_sidechain_link(uint32_t sourceId, uint32_t destId,
                                             uint32_t pluginIndex, uint32_t tapPoint,
                                             bool enabled) {
    return hirari_routing_edit_set_sidechain_link(this, sourceId, destId, pluginIndex, tapPoint,
                                                  enabled);
}

void HirariUnifiedEngine::handle_midi_cc(uint8_t channel, uint8_t cc,
                                       uint8_t value) noexcept {
    hirari_midi_learn_handle_cc(m_midiLearnState, channel, cc, value);
}

void HirariUnifiedEngine::handle_midi_cc14(uint8_t channel, uint16_t controller,
                                         uint16_t value) noexcept {
    hirari_midi_learn_handle_cc14(m_midiLearnState, channel, controller, value);
}
void HirariUnifiedEngine::set_preview_synth_engine(uint32_t engine) {
    hirari_preview_synth_set_engine(m_previewSynthState, engine);
}
// Routing edit host callbacks. Rust decides every edit and the undo capture;
// these handlers only apply the resolved mutation to the live objects, under
// the same locks the previous per-method code took.
void HirariUnifiedEngine::installRoutingEditHost() {
    static const HirariRoutingHostOps ops = {
        [](void* host, uint32_t sourceId, uint32_t destId, HirariRoutingEdgeState* out) {
            if (!out) return false;
            void* routing = static_cast<HirariUnifiedEngine*>(host)->m_routingEngine;
            out->routeEnabled = hirari_routing_engine_has_connection(routing, sourceId, destId);
            out->routeGain = hirari_routing_engine_connection_gain(routing, sourceId, destId);
            out->sendGain = hirari_routing_engine_send_gain(routing, sourceId, destId);
            out->sendPreFader = hirari_routing_engine_send_pre_fader(routing, sourceId, destId);
            out->feedbackGain = hirari_routing_engine_feedback_gain(routing, sourceId, destId);
            return true;
        },
        [](void* host, uint32_t trackId, HirariRoutingTrackState* out) {
            if (!out) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            const Track* track = engine->findTrackLocked(trackId);
            if (!track) return false;
            out->armed = track->isRecordArmed();
            out->phaseInverted = track->isPhaseInverted();
            out->delaySamples = track->getTrackDelaySamples();
            out->isBus = dynamic_cast<const BusTrack*>(track) != nullptr;
            out->volume = track->getVolume();
            out->pan = track->getPan();
            out->muted = track->isMuted();
            out->solo = track->isSolo();
            return true;
        },
        [](void* host, HirariRoutingSidechainEntry* out, size_t capacity) {
            static_assert(sizeof(HirariRoutingSidechainEntry) == sizeof(HirariSidechainRoute));
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            return hirari_sidechain_routes_copy(
                engine->m_sidechainRoutes,
                reinterpret_cast<HirariSidechainRoute*>(out), capacity);
        },
        [](void* host, uint32_t sourceId, uint32_t destId,
           HirariRoutingSidechainContext* out) {
            if (!out) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            out->blockSize = engine->m_context && engine->m_context->blockSize > 0
                ? std::min(engine->m_context->blockSize,
                           ::Hirari::Core::EffectChain::kMaxSidechainBlockSize)
                : 512u;
            out->sampleRate = static_cast<uint32_t>(std::max(
                1.0, engine->m_sampleRate.load(std::memory_order_acquire)));
            out->generation = std::max<uint64_t>(
                1, engine->m_audioConfigGeneration.load(std::memory_order_acquire));
            Track* source = engine->findTrackLocked(sourceId);
            Track* destination = engine->findTrackLocked(destId);
            out->sourceFound = source != nullptr;
            out->destinationFound = destination != nullptr;
            out->sourcePrepared = source && destination && source->canProcess(out->blockSize);
            return true;
        },
        [](void* host, const HirariRoutingEditPayload* payload) {
            if (!payload) return false;
            void* routing = static_cast<HirariUnifiedEngine*>(host)->m_routingEngine;
            if (payload->op == kHirariRoutingEditSend) {
                if (payload->enabled) {
                    hirari_routing_engine_add_connection(
                        routing, payload->sourceId, payload->destId, payload->gain,
                        true, payload->preFader);
                } else {
                    hirari_routing_engine_remove_send(
                        routing, payload->sourceId, payload->destId);
                }
            } else if (payload->enabled) {
                hirari_routing_engine_add_connection(
                    routing, payload->sourceId, payload->destId,
                    payload->gain > 0.0f ? payload->gain : 1.0f, false, false);
            } else {
                hirari_routing_engine_remove_connection(
                    routing, payload->sourceId, payload->destId);
            }
            return true;
        },
        [](void* host, uint32_t sourceId, uint32_t destId, float gain) {
            void* routing = static_cast<HirariUnifiedEngine*>(host)->m_routingEngine;
            if (gain < 0.0f) {
                hirari_routing_engine_remove_feedback(routing, sourceId, destId);
                return true;
            }
            return hirari_routing_engine_add_feedback(routing, sourceId, destId, gain);
        },
        [](void* host, uint32_t trackId, int32_t op, bool enabled, uint32_t samples) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            Track* track = engine->findTrackLocked(trackId);
            if (!track) return false;
            if (op == kHirariRoutingEditArm) track->setRecordArmed(enabled);
            else if (op == kHirariRoutingEditPhase) track->setPhaseInverted(enabled);
            else if (op == kHirariRoutingEditMute) track->setMuted(enabled);
            else if (op == kHirariRoutingEditSolo) track->setSolo(enabled);
            else track->setTrackDelaySamples(samples);
            return true;
        },
        [](void* host, uint32_t destId, uint32_t pluginIndex) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            const bool removed = hirari_sidechain_engine_remove_link(
                engine->m_sidechainEngine, destId, pluginIndex);
            return hirari_sidechain_routes_remove_link(
                       engine->m_sidechainRoutes, destId, pluginIndex) || removed;
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
            auto& sourceBuffer = source->getWorkBuffer(context->blockSize);
            const float* left = sourceBuffer.getReadPointer(0);
            const float* right = sourceBuffer.getNumChannels() > 1
                ? sourceBuffer.getReadPointer(1) : left;
            // Registration copies one prepared block into Rust-owned storage;
            // supplying the frame count is part of that contract.
            return hirari_sidechain_engine_register_link(
                engine->m_sidechainEngine, destId, pluginIndex, sourceId, left, right,
                context->blockSize, context->sampleRate, context->generation, 1.0f,
                static_cast<uint8_t>(tapPoint));
        },
        [](void* host, uint32_t sourceId, uint32_t destId, uint32_t pluginIndex,
           uint32_t tapPoint) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            return hirari_sidechain_routes_set(engine->m_sidechainRoutes, sourceId, destId,
                                               pluginIndex, tapPoint);
        },
        [](void* host, uint32_t sourceId, uint32_t destId) {
            return hirari_routing_engine_add_dependency(
                static_cast<HirariUnifiedEngine*>(host)->m_routingEngine, sourceId, destId);
        },
        [](void* host, uint32_t sourceId, uint32_t destId) {
            hirari_routing_engine_remove_dependency(
                static_cast<HirariUnifiedEngine*>(host)->m_routingEngine, sourceId, destId);
            return true;
        },
        [](void* host) {
            hirari_routing_engine_build(
                static_cast<HirariUnifiedEngine*>(host)->m_routingEngine);
        },
        [](void* host) {
            static_cast<HirariUnifiedEngine*>(host)->mark_project_layout_changed();
        },
        [](void* host) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            hirari_routing_engine_build(engine->m_routingEngine);
            // syncStructuralChanges takes m_tracksMutex itself; Rust calls this
            // only after every mutation callback has released its lock.
            engine->syncStructuralChanges();
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
            if (!host) return -1;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            const Track* track = engine->findTrackLocked(trackId);
            if (!track) return -1;
            const std::string& name = track->getName();
            if (!out || capacity == 0) return static_cast<intptr_t>(name.size());
            const size_t written = std::min(capacity, name.size());
            std::memcpy(out, name.data(), written);
            return static_cast<intptr_t>(written);
        },
        [](void* host, uint32_t trackId, const uint8_t* name, size_t nameLen) {
            if (!host || (!name && nameLen > 0)) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            const std::string value =
                nameLen > 0 ? std::string(reinterpret_cast<const char*>(name), nameLen)
                            : std::string();
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            Track* track = engine->findTrackLocked(trackId);
            if (!track) return false;
            track->setName(value);
            return true;
        },
        nullptr,  // undo capture lives in the Rust control edit host.
        [](void* host, uint32_t trackId, bool enabled, uint32_t leftChannel,
           uint32_t rightChannel) -> bool {
            if (!host) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            if (!enabled) {
                if (engine->m_inputMonitorTrackId.load(std::memory_order_acquire) == trackId) {
                    engine->m_inputMonitorTrackId.store(kNoInputMonitorTrack,
                                                       std::memory_order_release);
                }
                return true;
            }
            {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                if (!engine->findTrackLocked(trackId)) return false;
            }
            // One physical input is monitored at a time. Enabling another track
            // atomically moves the monitor focus without touching record-arm state.
            const uint64_t channelPair =
                (static_cast<uint64_t>(rightChannel) << 32) | leftChannel;
            engine->m_inputMonitorChannelPair.store(channelPair, std::memory_order_release);
            engine->m_inputMonitorTrackId.store(trackId, std::memory_order_release);
            return true;
        },
        [](void* host, uint32_t macroIndex, float value) {
            if (!host) return;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            hirari_macro_control_set_value(engine->m_macroControl, macroIndex, value);
        },
        [](void* host, uint32_t macroIndex, uint8_t channel, bool cc14, uint16_t controller,
           float minimum, float maximum, float curve, bool pickup) {
            if (!host) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            const void* targetState = engine->m_macroControl;
            if (targetState == nullptr) return false;
            if (cc14) {
                hirari_midi_learn_add_mapping_14bit(
                    engine->m_midiLearnState, controller, channel, targetState, macroIndex,
                    minimum, maximum, curve, pickup);
            } else {
                hirari_midi_learn_add_mapping(
                    engine->m_midiLearnState, static_cast<uint8_t>(controller), channel,
                    targetState, macroIndex, minimum, maximum, curve, pickup);
            }
            return true;
        },
        [](void* host) -> void* {
            return host ? static_cast<HirariUnifiedEngine*>(host)->undo_state() : nullptr;
        },
    };
    hirari_control_edit_host_install(&ops);
}

// Plugin-chain edit host callbacks. Rust decides whether an edit is admitted
// and which undo entry is recorded; these handlers only read or apply.
void HirariUnifiedEngine::installPluginEditHost() {
    static const HirariPluginEditHostOps ops = {
        [](void* host, uint32_t trackId, uint32_t pluginIndex) -> int32_t {
            if (!host) return -1;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            const Track* track = engine->findTrackLocked(trackId);
            return track ? (track->getPluginBypass(pluginIndex) ? 1 : 0) : -1;
        },
        [](void* host, uint32_t trackId, uint32_t pluginIndex, bool bypassed) {
            if (!host) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            Track* track = engine->findTrackLocked(trackId);
            return track && track->setPluginBypass(pluginIndex, bypassed);
        },
        [](void* host, uint32_t trackId, uint32_t fromIndex, uint32_t toIndex) {
            if (!host) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            Track* track = engine->findTrackLocked(trackId);
            return track && track->movePlugin(fromIndex, toIndex);
        },
        nullptr,  // undo capture lives in the Rust plugin edit host.
        nullptr,  // undo capture lives in the Rust plugin edit host.
        [](void* host, uint32_t trackId, uint32_t pluginIndex, uint32_t parameterId,
           float* out) -> bool {
            if (!host || !out) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            const Track* track = engine->findTrackLocked(trackId);
            if (!track) return false;
            *out = track->getPluginParameter(pluginIndex, parameterId);
            return true;
        },
        [](void* host, uint32_t trackId, uint32_t pluginIndex, uint32_t parameterId,
           float value) -> bool {
            if (!host) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            Track* track = engine->findTrackLocked(trackId);
            return track && track->setPluginParameter(pluginIndex, parameterId, value);
        },
        nullptr,  // undo capture lives in the Rust plugin edit host.
        [](void* host, uint32_t trackId, uint32_t pluginIndex, uint32_t* type,
           uint8_t* out, size_t capacity) -> size_t {
            if (!host) return std::numeric_limits<size_t>::max();
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            const Track* track = engine->findTrackLocked(trackId);
            if (!track) return std::numeric_limits<size_t>::max();
            const auto& types = track->getPluginTypes();
            const auto states = track->getPluginStates();
            if (pluginIndex >= types.size() || pluginIndex >= states.size())
                return std::numeric_limits<size_t>::max();
            const auto& state = states[pluginIndex];
            if (type) *type = types[pluginIndex];
            if (out && capacity >= state.size() && !state.empty()) {
                std::memcpy(out, state.data(), state.size());
            }
            return state.size();
        },
        [](void* host, uint32_t trackId, uint32_t pluginIndex, const uint8_t* state,
           size_t size, const uint8_t*, size_t, bool) -> bool {
            if (!host || (size != 0 && !state)) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            Track* track = engine->findTrackLocked(trackId);
            if (!track) return false;
            std::vector<uint8_t> replacement;
            if (size != 0) replacement.assign(state, state + size);
            return track->setPluginState(pluginIndex, replacement);
        },
        [](void* host, uint32_t trackId, uint32_t pluginType, const uint8_t* path, size_t pathLength,
           bool prepare, uint32_t* outIndex) -> bool {
            if (!host || !outIndex || (pathLength != 0 && !path)) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            Track* track = engine->findTrackLocked(trackId);
            if (!track) return false;
            const bool sandboxed = path != nullptr;
            const bool inserted =
                sandboxed ? track->addSandboxedPlugin(std::string(
                                reinterpret_cast<const char*>(path), pathLength))
                          : track->addPlugin(pluginType);
            if (!inserted) return false;
            if (prepare && engine->m_context) {
                track->prepareToPlay(engine->m_context->sampleRate, engine->m_context->blockSize);
            }
            *outIndex = static_cast<uint32_t>(track->getPluginCount() - 1);
            return true;
        },
        [](void* host, uint32_t trackId, uint32_t pluginIndex, void** outSnapshot) -> bool {
            if (!host) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            Track* track = engine->findTrackLocked(trackId);
            if (!track) return false;
            if (!outSnapshot) return track->removePlugin(pluginIndex);
            auto* snapshot = new Track::PluginSnapshot();
            if (track->removePlugin(pluginIndex, snapshot)) {
                *outSnapshot = snapshot;
                return true;
            }
            delete snapshot;
            return false;
        },
        [](void* host, uint32_t trackId, uint32_t pluginIndex, void* snapshot) -> bool {
            if (!host || !snapshot) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            Track* track = engine->findTrackLocked(trackId);
            return track && track->restorePlugin(
                pluginIndex, *static_cast<const Track::PluginSnapshot*>(snapshot));
        },
        [](void* snapshot) { delete static_cast<Track::PluginSnapshot*>(snapshot); },
        &HirariUnifiedEngine::recordNativeReplay,
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
        m_sidechainEngine, destId, pluginIndex, sourceId);
}
