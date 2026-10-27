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
    m_midiLearnManager.handleMidiCC(channel, cc, value);
}

void HirariUnifiedEngine::handle_midi_cc14(uint8_t channel, uint16_t controller,
                                         uint16_t value) noexcept {
    m_midiLearnManager.handleMidiCC14(channel, controller, value);
}
void HirariUnifiedEngine::set_preview_synth_engine(uint32_t engine) {
    m_previewSynth.setEngine(engine);
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
            auto& routing = static_cast<HirariUnifiedEngine*>(host)->m_routingEngine;
            out->routeEnabled = routing.hasConnection(sourceId, destId);
            out->routeGain = routing.connectionGain(sourceId, destId);
            out->sendGain = routing.sendConnectionGain(sourceId, destId);
            out->sendPreFader = routing.sendPreFader(sourceId, destId);
            out->feedbackGain = routing.feedbackConnectionGain(sourceId, destId);
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
            auto& routing = static_cast<HirariUnifiedEngine*>(host)->m_routingEngine;
            if (payload->op == kHirariRoutingEditSend) {
                if (payload->enabled) {
                    routing.addConnection(payload->sourceId, payload->destId, payload->gain,
                                          true, payload->preFader);
                } else {
                    routing.removeSendConnection(payload->sourceId, payload->destId);
                }
            } else if (payload->enabled) {
                routing.addConnection(payload->sourceId, payload->destId,
                                      payload->gain > 0.0f ? payload->gain : 1.0f);
            } else {
                routing.removeConnection(payload->sourceId, payload->destId);
            }
            return true;
        },
        [](void* host, uint32_t sourceId, uint32_t destId, float gain) {
            auto& routing = static_cast<HirariUnifiedEngine*>(host)->m_routingEngine;
            if (gain < 0.0f) {
                routing.removeFeedbackConnection(sourceId, destId);
                return true;
            }
            return routing.addFeedbackConnection(sourceId, destId, gain);
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
            return static_cast<HirariUnifiedEngine*>(host)->m_routingEngine
                .addProcessingDependency(sourceId, destId);
        },
        [](void* host, uint32_t sourceId, uint32_t destId) {
            static_cast<HirariUnifiedEngine*>(host)->m_routingEngine
                .removeProcessingDependency(sourceId, destId);
            return true;
        },
        [](void* host) {
            static_cast<HirariUnifiedEngine*>(host)->m_routingEngine.buildGraph();
        },
        [](void* host) {
            static_cast<HirariUnifiedEngine*>(host)->mark_project_layout_changed();
        },
        [](void* host) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            engine->m_routingEngine.buildGraph();
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
                engine->m_midiLearnManager.addMapping14Bit(
                    controller, channel, targetState, macroIndex, minimum, maximum, curve,
                    pickup);
            } else {
                engine->m_midiLearnManager.addMappingAdvanced(
                    static_cast<uint8_t>(controller), channel, targetState, macroIndex,
                    minimum, maximum, curve, pickup);
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
    // The bridge passes an absolute sample position as a double for ABI
    // compatibility. Reject fractional positions instead of truncating them
    // and silently moving an edit by part of a sample.
    if (!std::isfinite(splitBeat) || splitBeat < 0.0 || std::floor(splitBeat) != splitBeat) return 0;
    std::shared_ptr<Track> target;
    Region before;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() != tid) continue;
            for (const auto& region : track->getRegions()) {
                if (region.id != rid) continue;
                target = track;
                before = region;
                break;
            }
            break;
        }
    }
    if (!target || before.len == 0 || before.start > UINT64_MAX - before.len) return 0;
    const double end = static_cast<double>(before.start) + static_cast<double>(before.len);
    if (splitBeat <= static_cast<double>(before.start) || splitBeat >= end) return 0;
    const uint64_t splitOffset = static_cast<uint64_t>(splitBeat - static_cast<double>(before.start));
    if (splitOffset == 0 || splitOffset >= before.len) return 0;

    // Rust owns the split policy: geometry, fades, source offsets, the warp
    // map split and the local range/comp windows all come from here. This
    // function keeps the payload (audio buffer, note store, caches) and
    // applies the returned state.
    static_assert(sizeof(Region::WarpMarker) == sizeof(HirariWarpMarker));
    static_assert(sizeof(Region::RangeEdit) == sizeof(HirariRegionRangeEdit));
    static_assert(sizeof(Region::CompRange) == sizeof(HirariRegionCompRange));
    HirariRegionSplitInput input{};
    input.state = before.editableState();
    input.splitOffset = splitOffset;
    input.markers = reinterpret_cast<const HirariWarpMarker*>(before.warpMarkers.data());
    input.markerCount = before.warpMarkers.size();
    input.ranges = reinterpret_cast<const HirariRegionRangeEdit*>(before.rangeEdits.data());
    input.rangeCount = before.rangeEdits.size();
    input.compRanges = reinterpret_cast<const HirariRegionCompRange*>(before.compRanges.data());
    input.compRangeCount = before.compRanges.size();
    HirariRegionSplitOutput plan{};
    if (!hirari_region_split_plan(&input, &plan)) return 0;

    Region left = before;
    Region right = before;
    left.applyEditableState(plan.left);
    right.applyEditableState(plan.right);
    left.warpMarkers.clear();
    right.warpMarkers.clear();
    if (plan.leftMarkerCount != 0) {
        left.warpMarkers.assign(plan.leftMarkers,
                                plan.leftMarkers + plan.leftMarkerCount);
    }
    if (plan.rightMarkerCount != 0) {
        right.warpMarkers.assign(plan.rightMarkers,
                                 plan.rightMarkers + plan.rightMarkerCount);
    }
    if (plan.leftRangeCount != 0) {
        left.rangeEdits.assign(
            reinterpret_cast<const Region::RangeEdit*>(plan.leftRanges),
            reinterpret_cast<const Region::RangeEdit*>(plan.leftRanges) + plan.leftRangeCount);
    } else {
        left.rangeEdits.clear();
    }
    if (plan.rightRangeCount != 0) {
        right.rangeEdits.assign(
            reinterpret_cast<const Region::RangeEdit*>(plan.rightRanges),
            reinterpret_cast<const Region::RangeEdit*>(plan.rightRanges) + plan.rightRangeCount);
    } else {
        right.rangeEdits.clear();
    }
    if (plan.leftCompRangeCount != 0) {
        left.compRanges.assign(
            reinterpret_cast<const Region::CompRange*>(plan.leftCompRanges),
            reinterpret_cast<const Region::CompRange*>(plan.leftCompRanges) + plan.leftCompRangeCount);
    } else {
        left.compRanges.clear();
    }
    if (plan.rightCompRangeCount != 0) {
        right.compRanges.assign(
            reinterpret_cast<const Region::CompRange*>(plan.rightCompRanges),
            reinterpret_cast<const Region::CompRange*>(plan.rightCompRanges) + plan.rightCompRangeCount);
    } else {
        right.compRanges.clear();
    }

    // Pitch/formant note edits use region-relative seconds. Split each segment
    // at the same timeline boundary and move the right half back to time zero;
    // copying the original store to both clips applies stale edits to
    // unrelated audio after a split. Rust windows the segments and rebuilds
    // the boundary anchors from the source curve.
    left.noteEdits = ::hirari::editing::RegionNoteEdits{};
    right.noteEdits = ::hirari::editing::RegionNoteEdits{};
    const double sampleRate = m_sampleRate.load(std::memory_order_acquire);
    // Prefer the immutable published projection: a concurrent note edit must
    // not mutate the source table while Rust windows it. Fall back to the live
    // store only for a region that was never published.
    const auto* noteSnapshot = before.noteEdits.frozenHandle();
    const bool notesSplit = noteSnapshot
        ? hirari_region_vari_audio_frozen_split(
              noteSnapshot, left.noteEdits.storeHandle(),
              right.noteEdits.storeHandle(), sampleRate, splitOffset, before.len)
        : hirari_region_vari_audio_store_split(
              before.noteEdits.storeHandle(), left.noteEdits.storeHandle(),
              right.noteEdits.storeHandle(), sampleRate, splitOffset, before.len);
    hirari_region_split_plan_free(&plan);
    if (!notesSplit) return 0;

    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        right.id = allocateRegionIdLocked();
    }
    if (right.id == 0) return 0;

    const auto publish = [this, target, rid, left, right]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->removeRegion(rid);
        target->addRegion(left);
        target->addRegion(right);
        hirari_region_waveform_forget(this, rid);
        hirari_region_waveform_forget(this, right.id);
    };
    const uint32_t rightId = right.id;
    const auto restore = [this, target, rid, rightId, before]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->removeRegion(rid);
        target->removeRegion(rightId);
        target->addRegion(before);
        hirari_region_waveform_forget(this, rid);
    };
    m_undoManager.performAction("Split Region", restore, publish);
    return rightId;
}

uint32_t HirariUnifiedEngine::duplicate_region(uint32_t tid, uint32_t rid, uint64_t startSample) {
    std::shared_ptr<Track> target;
    Region duplicate;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
        if (track->getId() != tid) continue;
        for (const auto& region : track->getRegions()) {
            if (region.id != rid) continue;
            // Rust owns the duplicate policy: the loop-span overflow check,
            // the source-trim ceiling and the backing-up of baseStart.
            const auto sourceState = region.editableState();
            HirariRegionState duplicateState{};
            if (!hirari_region_duplicate_state(&sourceState, startSample,
                                               &duplicateState)) continue;
            target = track;
            duplicate = region;
            duplicate.applyEditableState(duplicateState);
            duplicate.id = allocateRegionIdLocked();
            if (duplicate.id == 0) return 0;
            break;
        }
        if (target) break;
        }
    }
    if (!target) return 0;

    // A duplicated clip is a new timeline instance. Preserve the source
    // offset, trim, fades, warp, pitch, loop and shared audio asset, but keep
    // one stable identity across undo/redo instead of allocating a new ID on
    // every redo.
    const uint32_t duplicateId = duplicate.id;
    const auto apply = [this, target, duplicate]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->addRegion(duplicate);
    };
    const auto remove = [this, target, duplicateId]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->removeRegion(duplicateId);
        hirari_region_waveform_forget(this, duplicateId);
    };
    m_undoManager.performAction(
        "Duplicate Region", remove, apply);
    return duplicateId;
}
