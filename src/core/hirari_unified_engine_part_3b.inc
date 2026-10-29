bool HirariUnifiedEngine::get_plugin_bypass(uint32_t tid, uint32_t pluginIndex) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track->getId() == tid) return track->getPluginBypass(pluginIndex);
    }
    return false;
}
bool HirariUnifiedEngine::move_plugin(uint32_t tid, uint32_t fromIndex, uint32_t toIndex) {
    if (!hirari_plugin_edit_move(this, tid, fromIndex, toIndex)) return false;
    syncStructuralChanges();
    return true;
}
bool HirariUnifiedEngine::set_phase_invert(uint32_t tid, bool inverted) {
    return hirari_routing_edit_set_phase_invert(this, tid, inverted);
}
bool HirariUnifiedEngine::set_track_delay_samples(uint32_t tid, uint32_t samples) {
    return hirari_routing_edit_set_track_delay_samples(this, tid, samples);
}
uint32_t HirariUnifiedEngine::get_track_delay_samples(uint32_t tid) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid) return track->getTrackDelaySamples();
    }
    return 0;
}
bool HirariUnifiedEngine::set_track_armed(uint32_t tid, bool armed) {
    return hirari_routing_edit_set_track_armed(this, tid, armed);
}

bool HirariUnifiedEngine::set_track_input_monitor(uint32_t tid, bool enabled) {
    return set_track_input_monitor_channels(tid, enabled, 0, 1);
}

bool HirariUnifiedEngine::set_track_input_monitor_channels(
    uint32_t tid, bool enabled, uint32_t leftChannel, uint32_t rightChannel) {
    return hirari_control_edit_set_input_monitor(
        this, tid, enabled, leftChannel, rightChannel);
}

void HirariUnifiedEngine::set_macro_value(uint32_t macroIndex, float value) {
    hirari_control_edit_set_macro_value(this, macroIndex, value);
}
bool HirariUnifiedEngine::bind_midi_cc_to_macro(uint8_t channel, uint8_t cc,
                                               uint32_t macroIndex, float minimum,
                                               float maximum, float curve,
                                               bool pickup) noexcept {
    return hirari_control_edit_bind_midi_cc(
        this, channel, cc, macroIndex, minimum, maximum, curve, pickup);
}
bool HirariUnifiedEngine::bind_midi_cc14_to_macro(uint8_t channel, uint16_t controller,
                                                 uint32_t macroIndex, float minimum,
                                                 float maximum, float curve,
                                                 bool pickup) noexcept {
    return hirari_control_edit_bind_midi_cc14(
        this, channel, controller, macroIndex, minimum, maximum, curve, pickup);
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
bool HirariUnifiedEngine::set_route(uint32_t sourceId, uint32_t destId, bool enabled) {
    return hirari_routing_edit_set_route(this, sourceId, destId, enabled);
}
bool HirariUnifiedEngine::set_route_gain(uint32_t sourceId, uint32_t destId, float gain,
                                         bool enabled) {
    return hirari_routing_edit_set_route_gain(this, sourceId, destId, gain, enabled);
}
bool HirariUnifiedEngine::set_send_route(uint32_t sourceId, uint32_t destId, float gain,
                                         bool preFader, bool enabled) {
    return hirari_routing_edit_set_send_route(this, sourceId, destId, gain, preFader, enabled);
}
bool HirariUnifiedEngine::set_feedback_route(uint32_t sourceId, uint32_t destId, float gain,
                                             bool enabled) {
    return hirari_routing_edit_set_feedback_route(this, sourceId, destId, gain, enabled);
}
bool HirariUnifiedEngine::set_sidechain_link(uint32_t sourceId, uint32_t destId,
                                             uint32_t pluginIndex, uint32_t tapPoint,
                                             bool enabled) {
    return hirari_routing_edit_set_sidechain_link(this, sourceId, destId, pluginIndex, tapPoint,
                                                  enabled);
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
            for (const auto& track : engine->m_tracks) {
                if (!track || track->getId() != trackId) continue;
                out->armed = track->isRecordArmed();
                out->phaseInverted = track->isPhaseInverted();
                out->delaySamples = track->getTrackDelaySamples();
                out->isBus = dynamic_cast<BusTrack*>(track.get()) != nullptr;
                out->volume = track->getVolume();
                out->pan = track->getPan();
                out->muted = track->isMuted();
                out->solo = track->isSolo();
                return true;
            }
            return false;
        },
        [](void* host, HirariRoutingSidechainEntry* out, size_t capacity) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            size_t count = 0;
            for (const auto& route : engine->m_sidechainRoutes) {
                if (out && count < capacity) {
                    out[count] = {route.sourceId, route.destinationId, route.pluginIndex,
                                  route.tapPoint};
                }
                ++count;
            }
            return count;
        },
        [](void* host, uint32_t sourceId, uint32_t destId,
           HirariRoutingSidechainContext* out) {
            if (!out) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            out->blockSize = engine->m_context && engine->m_context->blockSize > 0
                ? std::min(engine->m_context->blockSize,
                           Hirari::Core::Engine::SidechainManager::kMaxBlockSize)
                : 512u;
            out->sampleRate = static_cast<uint32_t>(std::max(
                1.0, engine->m_sampleRate.load(std::memory_order_acquire)));
            out->generation = std::max<uint64_t>(
                1, engine->m_audioConfigGeneration.load(std::memory_order_acquire));
            Track* source = nullptr;
            Track* destination = nullptr;
            for (const auto& track : engine->m_tracks) {
                if (!track) continue;
                if (track->getId() == sourceId) source = track.get();
                if (track->getId() == destId) destination = track.get();
            }
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
            for (auto& track : engine->m_tracks) {
                if (!track || track->getId() != trackId) continue;
                if (op == kHirariRoutingEditArm) track->setRecordArmed(enabled);
                else if (op == kHirariRoutingEditPhase) track->setPhaseInverted(enabled);
                else if (op == kHirariRoutingEditMute) track->setMuted(enabled);
                else if (op == kHirariRoutingEditSolo) track->setSolo(enabled);
                else track->setTrackDelaySamples(samples);
                return true;
            }
            return false;
        },
        [](void* host, uint32_t destId, uint32_t pluginIndex) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            const bool removed = engine->m_sidechainManager.removeLink(destId, pluginIndex);
            auto& routes = engine->m_sidechainRoutes;
            const size_t before = routes.size();
            routes.erase(
                std::remove_if(routes.begin(), routes.end(),
                               [destId, pluginIndex](const SidechainRoute& route) {
                                   return route.destinationId == destId &&
                                          route.pluginIndex == pluginIndex;
                               }),
                routes.end());
            return removed || routes.size() != before;
        },
        [](void* host, uint32_t destId, uint32_t pluginIndex, uint32_t sourceId,
           const HirariRoutingSidechainContext* context, uint32_t tapPoint, bool silent) {
            if (!context) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            const auto tap = static_cast<Hirari::Core::Engine::SidechainTapPoint>(tapPoint);
            if (silent) {
                return engine->m_sidechainManager.registerSilentLink(
                    destId, pluginIndex, sourceId, context->blockSize, context->sampleRate,
                    context->generation, 1.0f, tap);
            }
            Track* source = nullptr;
            for (const auto& track : engine->m_tracks) {
                if (track && track->getId() == sourceId) source = track.get();
            }
            if (!source) return false;
            auto& sourceBuffer = source->getWorkBuffer(context->blockSize);
            Hirari::Core::Engine::SidechainLink link;
            link.sourceBufferL = sourceBuffer.getReadPointer(0);
            link.sourceBufferR = sourceBuffer.getNumChannels() > 1
                ? sourceBuffer.getReadPointer(1) : link.sourceBufferL;
            link.sourceTrackId = sourceId;
            link.sourceSampleRate = context->sampleRate;
            link.sourceGeneration = context->generation;
            link.level = 1.0f;
            link.tapPoint = tap;
            // Registration copies one prepared block into manager-owned
            // storage; supplying the frame count is part of that contract.
            link.sourceFrames = context->blockSize;
            return engine->m_sidechainManager.registerLink(destId, pluginIndex, link);
        },
        [](void* host, uint32_t sourceId, uint32_t destId, uint32_t pluginIndex,
           uint32_t tapPoint) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (auto& route : engine->m_sidechainRoutes) {
                if (route.destinationId == destId && route.pluginIndex == pluginIndex) {
                    route.sourceId = sourceId;
                    route.tapPoint = tapPoint;
                    return true;
                }
            }
            engine->m_sidechainRoutes.push_back({sourceId, destId, pluginIndex, tapPoint});
            return true;
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
        [](void* host, const char* name, const HirariRoutingEditPayload* undo, size_t undoCount,
           const HirariRoutingEditPayload* redo, size_t redoCount, bool perform, bool coalesce) {
            if (!host || !name) return;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::vector<HirariRoutingEditPayload> undoCopy;
            if (undo && undoCount) undoCopy.assign(undo, undo + undoCount);
            std::vector<HirariRoutingEditPayload> redoCopy;
            if (redo && redoCount) redoCopy.assign(redo, redo + redoCount);
            const auto applyAll = [host](const std::vector<HirariRoutingEditPayload>& entries) {
                hirari_routing_edit_apply_payloads(host, entries.data(), entries.size());
            };
            if (perform) {
                // performAction replays the redo through the Rust policy.
                engine->m_undoManager.performAction(
                    name, [applyAll, undoCopy] { applyAll(undoCopy); },
                    [applyAll, redoCopy] { applyAll(redoCopy); });
                return;
            }
            engine->m_undoManager.recordAppliedAction(
                name, [applyAll, undoCopy] { applyAll(undoCopy); },
                [applyAll, redoCopy] { applyAll(redoCopy); }, coalesce);
        },
        [](void* host, uint32_t trackId, int32_t op, float value) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (auto& track : engine->m_tracks) {
                if (!track || track->getId() != trackId) continue;
                if (op == kHirariRoutingEditVolume) track->setVolume(value);
                else if (op == kHirariRoutingEditPan) track->setPan(value);
                else return false;
                return true;
            }
            return false;
        },
    };
    hirari_routing_edit_host_install(&ops);
}

namespace {
[[maybe_unused]] const bool kRoutingEditHostInstalled = [] {
    HirariUnifiedEngine::installRoutingEditHost();
    return true;
}();
}  // namespace

// Control edit host callbacks. Rust validates every naming/monitor/macro edit
// and decides the undo entry; these handlers only read or apply the mutation.
void HirariUnifiedEngine::installControlEditHost() {
    static const HirariControlEditHostOps ops = {
        [](void* host, uint32_t trackId, uint8_t* out, size_t capacity) -> intptr_t {
            if (!host) return -1;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (const auto& track : engine->m_tracks) {
                if (!track || track->getId() != trackId) continue;
                const std::string& name = track->getName();
                if (!out || capacity == 0) return static_cast<intptr_t>(name.size());
                const size_t written = std::min(capacity, name.size());
                std::memcpy(out, name.data(), written);
                return static_cast<intptr_t>(written);
            }
            return -1;
        },
        [](void* host, uint32_t trackId, const uint8_t* name, size_t nameLen) {
            if (!host || (!name && nameLen > 0)) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            const std::string value =
                nameLen > 0 ? std::string(reinterpret_cast<const char*>(name), nameLen)
                            : std::string();
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (auto& track : engine->m_tracks) {
                if (!track || track->getId() != trackId) continue;
                track->setName(value);
                return true;
            }
            return false;
        },
        [](void* host, uint32_t trackId, const uint8_t* before, size_t beforeLen,
           const uint8_t* after, size_t afterLen) {
            if (!host) return;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::shared_ptr<Track> target;
            {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                for (auto& track : engine->m_tracks) {
                    if (track && track->getId() == trackId) {
                        target = track;
                        break;
                    }
                }
            }
            if (!target) return;
            const std::string beforeValue(
                before ? reinterpret_cast<const char*>(before) : "", before ? beforeLen : 0);
            const std::string afterValue(
                after ? reinterpret_cast<const char*>(after) : "", after ? afterLen : 0);
            const auto apply = [engine, target](const std::string& value) {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                target->setName(value);
            };
            engine->m_undoManager.recordAppliedAction(
                "Rename Track", [apply, beforeValue] { apply(beforeValue); },
                [apply, afterValue] { apply(afterValue); });
        },
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
                const bool exists = std::any_of(
                    engine->m_tracks.begin(), engine->m_tracks.end(),
                    [trackId](const auto& track) {
                        return track && track->getId() == trackId;
                    });
                if (!exists) return false;
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
    };
    hirari_control_edit_host_install(&ops);
}

namespace {
[[maybe_unused]] const bool kControlEditHostInstalled = [] {
    HirariUnifiedEngine::installControlEditHost();
    return true;
}();
}  // namespace

// Plugin-chain edit host callbacks. Rust decides whether an edit is admitted
// and which undo entry is recorded; these handlers only read or apply.
void HirariUnifiedEngine::installPluginEditHost() {
    static const HirariPluginEditHostOps ops = {
        [](void* host, uint32_t trackId, uint32_t pluginIndex) -> int32_t {
            if (!host) return -1;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (const auto& track : engine->m_tracks) {
                if (!track || track->getId() != trackId) continue;
                return track->getPluginBypass(pluginIndex) ? 1 : 0;
            }
            return -1;
        },
        [](void* host, uint32_t trackId, uint32_t pluginIndex, bool bypassed) {
            if (!host) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (auto& track : engine->m_tracks) {
                if (!track || track->getId() != trackId) continue;
                return track->setPluginBypass(pluginIndex, bypassed);
            }
            return false;
        },
        [](void* host, uint32_t trackId, uint32_t fromIndex, uint32_t toIndex) {
            if (!host) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (auto& track : engine->m_tracks) {
                if (!track || track->getId() != trackId) continue;
                return track->movePlugin(fromIndex, toIndex);
            }
            return false;
        },
        [](void* host, uint32_t trackId, uint32_t pluginIndex, bool before, bool after) {
            if (!host) return;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::shared_ptr<Track> target;
            {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                for (auto& track : engine->m_tracks) {
                    if (track && track->getId() == trackId) {
                        target = track;
                        break;
                    }
                }
            }
            if (!target) return;
            // Undo replays the stored values without re-entering the policy,
            // exactly as the previous engine-owned lambdas did.
            const auto undo = [target, pluginIndex, before]() {
                (void)target->setPluginBypass(pluginIndex, before);
            };
            const auto redo = [target, pluginIndex, after]() {
                (void)target->setPluginBypass(pluginIndex, after);
            };
            engine->m_undoManager.recordAppliedAction("Set Plugin Bypass", undo, redo);
        },
        [](void* host, uint32_t trackId, uint32_t fromIndex, uint32_t toIndex) {
            if (!host) return;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::shared_ptr<Track> target;
            {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                for (auto& track : engine->m_tracks) {
                    if (track && track->getId() == trackId) {
                        target = track;
                        break;
                    }
                }
            }
            if (!target) return;
            const auto undo = [target, fromIndex, toIndex]() {
                (void)target->movePlugin(toIndex, fromIndex);
            };
            const auto redo = [target, fromIndex, toIndex]() {
                (void)target->movePlugin(fromIndex, toIndex);
            };
            engine->m_undoManager.recordAppliedAction("Move Plugin", undo, redo);
        },
        [](void* host, uint32_t trackId, uint32_t pluginIndex, uint32_t parameterId,
           float* out) -> bool {
            if (!host || !out) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (const auto& track : engine->m_tracks) {
                if (!track || track->getId() != trackId) continue;
                *out = track->getPluginParameter(pluginIndex, parameterId);
                return true;
            }
            return false;
        },
        [](void* host, uint32_t trackId, uint32_t pluginIndex, uint32_t parameterId,
           float value) -> bool {
            if (!host) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (auto& track : engine->m_tracks) {
                if (!track || track->getId() != trackId) continue;
                return track->setPluginParameter(pluginIndex, parameterId, value);
            }
            return false;
        },
        [](void* host, uint32_t trackId, uint32_t pluginIndex, uint32_t parameterId,
           float before, float after) {
            if (!host) return;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::shared_ptr<Track> target;
            {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                for (auto& track : engine->m_tracks) {
                    if (track && track->getId() == trackId) {
                        target = track;
                        break;
                    }
                }
            }
            if (!target) return;
            const auto undo = [target, pluginIndex, parameterId, before]() {
                (void)target->setPluginParameter(pluginIndex, parameterId, before);
            };
            const auto redo = [target, pluginIndex, parameterId, after]() {
                (void)target->setPluginParameter(pluginIndex, parameterId, after);
            };
            engine->m_undoManager.recordAppliedAction("Set Plugin Parameter", undo, redo);
        },
        [](void* host, uint32_t trackId, uint32_t pluginIndex, uint32_t* type,
           uint8_t* out, size_t capacity) -> size_t {
            if (!host) return std::numeric_limits<size_t>::max();
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (const auto& track : engine->m_tracks) {
                if (!track || track->getId() != trackId) continue;
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
            }
            return std::numeric_limits<size_t>::max();
        },
        [](void* host, uint32_t trackId, uint32_t pluginIndex, const uint8_t* state,
           size_t size, const uint8_t* before, size_t beforeSize, bool record) -> bool {
            if (!host || (size != 0 && !state)) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::shared_ptr<Track> target;
            bool found = false;
            {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                for (auto& track : engine->m_tracks) {
                    if (!track || track->getId() != trackId) continue;
                    found = true;
                    std::vector<uint8_t> replacement;
                    if (size != 0) replacement.assign(state, state + size);
                    if (!track->setPluginState(pluginIndex, replacement)) return false;
                    if (record) target = track;
                    break;
                }
            }
            if (!found) return false;
            if (!record) return true;
            // The state is already stored; the entry only replays the two
            // snapshots, exactly as the engine-owned lambdas did.
            std::vector<uint8_t> beforeState;
            std::vector<uint8_t> afterState;
            if (beforeSize != 0) beforeState.assign(before, before + beforeSize);
            if (size != 0) afterState.assign(state, state + size);
            const auto undo = [target, pluginIndex, beforeState]() {
                (void)target->setPluginState(pluginIndex, beforeState);
            };
            const auto redo = [target, pluginIndex, afterState]() {
                (void)target->setPluginState(pluginIndex, afterState);
            };
            engine->m_undoManager.recordAppliedAction("Load Plugin Preset", undo, redo);
            return true;
        },
        [](void* host, uint32_t trackId, uint32_t pluginType, const uint8_t* path, size_t pathLength,
           bool prepare, uint32_t* outIndex) -> bool {
            if (!host || !outIndex || (pathLength != 0 && !path)) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (auto& track : engine->m_tracks) {
                if (!track || track->getId() != trackId) continue;
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
            }
            return false;
        },
        [](void* host, uint32_t trackId, uint32_t pluginIndex, void** outSnapshot) -> bool {
            if (!host) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (auto& track : engine->m_tracks) {
                if (!track || track->getId() != trackId) continue;
                if (!outSnapshot) return track->removePlugin(pluginIndex);
                auto* snapshot = new Track::PluginSnapshot();
                if (track->removePlugin(pluginIndex, snapshot)) {
                    *outSnapshot = snapshot;
                    return true;
                }
                delete snapshot;
                return false;
            }
            return false;
        },
        [](void* host, uint32_t trackId, uint32_t pluginIndex, void* snapshot) -> bool {
            if (!host || !snapshot) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (auto& track : engine->m_tracks) {
                if (track && track->getId() == trackId)
                    return track->restorePlugin(
                        pluginIndex, *static_cast<const Track::PluginSnapshot*>(snapshot));
            }
            return false;
        },
        [](void* snapshot) { delete static_cast<Track::PluginSnapshot*>(snapshot); },
        &HirariUnifiedEngine::recordNativeReplay,
        &HirariUnifiedEngine::syncNative,
    };
    hirari_plugin_edit_host_install(&ops);
}

namespace {
[[maybe_unused]] const bool kPluginEditHostInstalled = [] {
    HirariUnifiedEngine::installPluginEditHost();
    return true;
}();
}  // namespace


bool HirariUnifiedEngine::has_sidechain_link(uint32_t sourceId, uint32_t destId,
                                           uint32_t pluginIndex) const {
    return m_sidechainManager.hasLink(
        destId, pluginIndex, sourceId);
}
bool HirariUnifiedEngine::split_region(uint32_t tid, uint32_t rid, double splitBeat) {
    return split_region_with_right_id(tid, rid, splitBeat) != 0;
}

uint32_t HirariUnifiedEngine::split_region_with_right_id(uint32_t tid, uint32_t rid, double splitBeat) {
    return hirari_region_split(this, tid, rid, splitBeat);
}

uint32_t HirariUnifiedEngine::duplicate_region(uint32_t tid, uint32_t rid, uint64_t startSample) {
    return hirari_region_duplicate(this, tid, rid, startSample);
}
