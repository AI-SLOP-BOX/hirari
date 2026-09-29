                if (t->setPluginBypass(pluginIndex, bypassed)) target = t;
                break;
            }
        }
    }
    // Bypass changes alter the active latency path. Rebuild the published
    // routing/PDC state after releasing m_tracksMutex; syncStructuralChanges
    // takes the same lock and must never be called while it is held.
    if (!target) return false;
    const auto undo = [target, pluginIndex, before]() {
        (void)target->setPluginBypass(pluginIndex, before);
    };
    const auto redo = [target, pluginIndex, bypassed]() {
        (void)target->setPluginBypass(pluginIndex, bypassed);
    };
    m_undoManager.recordAppliedAction("Set Plugin Bypass", undo, redo);
    syncStructuralChanges();
    return true;
}
bool HirariUnifiedEngine::get_plugin_bypass(uint32_t tid, uint32_t pluginIndex) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track->getId() == tid) return track->getPluginBypass(pluginIndex);
    }
    return false;
}
bool HirariUnifiedEngine::move_plugin(uint32_t tid, uint32_t fromIndex, uint32_t toIndex) {
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track && track->getId() == tid) {
                if (track->movePlugin(fromIndex, toIndex)) target = track;
                break;
            }
        }
    }
    if (!target) return false;
    const auto undo = [target, fromIndex, toIndex]() {
        (void)target->movePlugin(toIndex, fromIndex);
    };
    const auto redo = [target, fromIndex, toIndex]() {
        (void)target->movePlugin(fromIndex, toIndex);
    };
    m_undoManager.recordAppliedAction("Move Plugin", undo, redo);
    syncStructuralChanges();
    return true;
}
bool HirariUnifiedEngine::set_phase_invert(uint32_t tid, bool inverted) {
    std::shared_ptr<Track> target;
    bool before = false;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track && track->getId() == tid) {
                target = track;
                before = track->isPhaseInverted();
                break;
            }
        }
    }
    // A valid no-op is still a successful state application. Hydration and
    // idempotent command replay use the setter as an acknowledgement that the
    // target existed; rejecting the already-correct value made project restore
    // fail for every track whose phase state was false.
    if (!target) return false;
    if (before == inverted) return true;
    target->setPhaseInverted(inverted);
    const auto apply = [target](bool next) { target->setPhaseInverted(next); };
    m_undoManager.recordAppliedAction(
        "Phase Invert", [apply, before] { apply(before); },
        [apply, inverted] { apply(inverted); });
    return true;
}
bool HirariUnifiedEngine::set_track_delay_samples(uint32_t tid, uint32_t samples) {
    constexpr uint32_t kMaxTrackDelaySamples = 8192;
    if (samples > kMaxTrackDelaySamples) return false;
    std::shared_ptr<Track> target;
    uint32_t before = 0;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track && track->getId() == tid) {
            target = track;
            before = track->getTrackDelaySamples();
            break;
        }
    }
    if (!target) return false;
    target->setTrackDelaySamples(samples);
    const auto apply = [target](uint32_t next) { target->setTrackDelaySamples(next); };
    m_undoManager.performAction(
        "Track Delay " + std::to_string(tid),
        [apply, before] { apply(before); },
        [apply, samples] { apply(samples); });
    // performAction executes redo, but keep the live value explicit because
    // coalescing and nested transactions are allowed to invoke callbacks.
    target->setTrackDelaySamples(samples);
    return true;
}
uint32_t HirariUnifiedEngine::get_track_delay_samples(uint32_t tid) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid) return track->getTrackDelaySamples();
    }
    return 0;
}
bool HirariUnifiedEngine::set_track_armed(uint32_t tid, bool armed) {
    std::shared_ptr<Track> target;
    bool before = false;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track && track->getId() == tid) {
                target = track;
                before = track->isRecordArmed();
                break;
            }
        }
    }
    // Match the other scalar setters: an already-correct value is a valid
    // idempotent application, not a missing-target failure.
    if (!target) return false;
    if (before == armed) return true;
    target->setRecordArmed(armed);
    const auto apply = [target](bool next) { target->setRecordArmed(next); };
    m_undoManager.recordAppliedAction(
        "Record Arm", [apply, before] { apply(before); },
        [apply, armed] { apply(armed); });
    return true;
}

bool HirariUnifiedEngine::set_track_input_monitor(uint32_t tid, bool enabled) {
    return set_track_input_monitor_channels(tid, enabled, 0, 1);
}

bool HirariUnifiedEngine::set_track_input_monitor_channels(
    uint32_t tid, bool enabled, uint32_t leftChannel, uint32_t rightChannel) {
    if (leftChannel >= 32 || rightChannel >= 32) return false;
    if (!enabled) {
        if (m_inputMonitorTrackId.load(std::memory_order_acquire) == tid) {
            m_inputMonitorTrackId.store(kNoInputMonitorTrack, std::memory_order_release);
        }
        return true;
    }
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    const bool exists = std::any_of(m_tracks.begin(), m_tracks.end(),
        [tid](const auto& track) { return track && track->getId() == tid; });
    if (!exists) return false;
    // One physical input is monitored at a time. Enabling another track
    // atomically moves the monitor focus without touching record-arm state.
    const uint64_t channelPair = (static_cast<uint64_t>(rightChannel) << 32) | leftChannel;
    m_inputMonitorChannelPair.store(channelPair, std::memory_order_release);
    m_inputMonitorTrackId.store(tid, std::memory_order_release);
    return true;
}

void HirariUnifiedEngine::set_macro_value(uint32_t macroIndex, float value) {
    if (!std::isfinite(value)) return;
    m_macroControlManager.setMacroValue(macroIndex, value);
}
bool HirariUnifiedEngine::bind_midi_cc_to_macro(uint8_t channel, uint8_t cc,
                                               uint32_t macroIndex, float minimum,
                                               float maximum, float curve,
                                               bool pickup) noexcept {
    if (macroIndex >= MacroControlManager::kMaxMacros || channel > 16 || cc > 127 ||
        !std::isfinite(minimum) || !std::isfinite(maximum) || minimum > maximum ||
        !std::isfinite(curve) || curve < -1.0f || curve > 1.0f) {
        return false;
    }
    const void* targetState = m_macroControlManager.midiLearnState();
    if (targetState == nullptr) return false;
    m_midiLearnManager.addMappingAdvanced(
        cc, channel, targetState, macroIndex, minimum, maximum, curve, pickup);
    return true;
}
bool HirariUnifiedEngine::bind_midi_cc14_to_macro(uint8_t channel, uint16_t controller,
                                                 uint32_t macroIndex, float minimum,
                                                 float maximum, float curve,
                                                 bool pickup) noexcept {
    if (macroIndex >= MacroControlManager::kMaxMacros || channel > 16 || controller > 16383 ||
        !std::isfinite(minimum) || !std::isfinite(maximum) || minimum > maximum ||
        !std::isfinite(curve) || curve < -1.0f || curve > 1.0f) {
        return false;
    }
    const void* targetState = m_macroControlManager.midiLearnState();
    if (targetState == nullptr) return false;
    m_midiLearnManager.addMapping14Bit(
        controller, channel, targetState, macroIndex, minimum, maximum, curve, pickup);
    return true;
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
    if (sourceId >= Hirari::Core::Engine::RoutingEngine::kMaxNodes ||
        destId >= Hirari::Core::Engine::RoutingEngine::kMaxNodes || sourceId == destId) return false;
    auto& routing = m_routingEngine;
    const bool before = routing.hasConnection(sourceId, destId);
    if (before == enabled) return false;
    if (enabled) routing.addConnection(sourceId, destId);
    else routing.removeConnection(sourceId, destId);
    if (routing.hasConnection(sourceId, destId) != enabled) return false;
    const auto apply = [&routing, sourceId, destId](bool next) {
        if (next) routing.addConnection(sourceId, destId);
        else routing.removeConnection(sourceId, destId);
        routing.buildGraph();
    };
    m_undoManager.recordAppliedAction(
        "Audio Route", [apply, before] { apply(before); },
        [apply, enabled] { apply(enabled); });
    routing.buildGraph();
    syncStructuralChanges();
    return true;
}
bool HirariUnifiedEngine::set_route_gain(uint32_t sourceId, uint32_t destId, float gain, bool enabled) {
    if (sourceId >= Hirari::Core::Engine::RoutingEngine::kMaxNodes ||
        destId >= Hirari::Core::Engine::RoutingEngine::kMaxNodes || sourceId == destId ||
        !std::isfinite(gain) || gain < 0.0f || gain > 2.0f || (enabled && gain <= 0.0f)) return false;
    auto& routing = m_routingEngine;
    const bool beforeEnabled = routing.hasConnection(sourceId, destId);
    const float beforeGain = routing.connectionGain(sourceId, destId);
    if (beforeEnabled == enabled && (!enabled || std::abs(beforeGain - gain) < 1.0e-6f)) {
        return false;
    }
    if (enabled) routing.addConnection(sourceId, destId, gain);
    else routing.removeConnection(sourceId, destId);
    if (routing.hasConnection(sourceId, destId) != enabled) return false;
    const auto apply = [&routing, sourceId, destId](float nextGain, bool nextEnabled) {
        if (nextEnabled) routing.addConnection(sourceId, destId, nextGain);
        else routing.removeConnection(sourceId, destId);
        routing.buildGraph();
    };
    m_undoManager.recordAppliedAction(
        "Audio Route Gain",
        [apply, beforeGain, beforeEnabled] { apply(beforeGain, beforeEnabled); },
        [apply, gain, enabled] { apply(gain, enabled); });
    routing.buildGraph();
    syncStructuralChanges();
    return true;
}
bool HirariUnifiedEngine::set_send_route(uint32_t sourceId, uint32_t destId, float gain,
                                         bool preFader, bool enabled) {
    if (sourceId >= Hirari::Core::Engine::RoutingEngine::kMaxNodes ||
        destId >= Hirari::Core::Engine::RoutingEngine::kMaxNodes || sourceId == destId ||
        !std::isfinite(gain) || gain < 0.0f || gain > 2.0f || (enabled && gain <= 0.0f)) {
        return false;
    }
    if (enabled) {
        std::lock_guard<std::mutex> trackLock(m_tracksMutex);
        const auto source = std::find_if(m_tracks.begin(), m_tracks.end(),
            [sourceId](const auto& track) { return track && track->getId() == sourceId; });
        const auto destination = std::find_if(m_tracks.begin(), m_tracks.end(),
            [destId](const auto& track) { return track && track->getId() == destId; });
        if (source == m_tracks.end() || destination == m_tracks.end() ||
            dynamic_cast<BusTrack*>(destination->get()) == nullptr) return false;
    }
    auto& routing = m_routingEngine;
    const float beforeGain = routing.sendConnectionGain(sourceId, destId);
    const bool beforeEnabled = beforeGain > 0.0f;
    const bool beforePreFader = routing.sendPreFader(sourceId, destId);
    if (beforeEnabled == enabled && (!enabled ||
        (std::abs(beforeGain - gain) < 1.0e-6f && beforePreFader == preFader))) {
        return false;
    }
    if (enabled) routing.addConnection(sourceId, destId, gain, true, preFader);
    else routing.removeSendConnection(sourceId, destId);
    if ((routing.sendConnectionGain(sourceId, destId) > 0.0f) != enabled) return false;
    const auto apply = [&routing, sourceId, destId](float nextGain, bool nextPreFader,
                                                   bool nextEnabled) {
        if (nextEnabled) routing.addConnection(sourceId, destId, nextGain, true, nextPreFader);
        else routing.removeSendConnection(sourceId, destId);
        routing.buildGraph();
    };
    m_undoManager.recordAppliedAction(
        "Audio Send",
        [apply, beforeGain, beforePreFader, beforeEnabled] {
            apply(beforeGain, beforePreFader, beforeEnabled);
        },
        [apply, gain, preFader, enabled] { apply(gain, preFader, enabled); });
    routing.buildGraph();
    syncStructuralChanges();
    return true;
}
bool HirariUnifiedEngine::set_feedback_route(uint32_t sourceId, uint32_t destId,
                                           float gain, bool enabled) {
    if (sourceId >= Hirari::Core::Engine::RoutingEngine::kMaxNodes ||
        destId >= Hirari::Core::Engine::RoutingEngine::kMaxNodes || sourceId == destId ||
        !std::isfinite(gain)) return false;
    auto& routing = m_routingEngine;
    const float beforeGain = routing.feedbackConnectionGain(sourceId, destId);
    const bool changed = enabled
        ? routing.addFeedbackConnection(sourceId, destId, gain)
        : (beforeGain >= 0.0f ? (routing.removeFeedbackConnection(sourceId, destId), true) : false);
    if (!changed) return false;
    mark_project_layout_changed();
    const auto apply = [this, &routing, sourceId, destId](float nextGain) {
        if (nextGain < 0.0f) routing.removeFeedbackConnection(sourceId, destId);
        else routing.addFeedbackConnection(sourceId, destId, nextGain);
        mark_project_layout_changed();
    };
    m_undoManager.recordAppliedAction(
        "Feedback Route", [apply, beforeGain] { apply(beforeGain); },
        [apply, gain, enabled] { apply(enabled ? std::clamp(gain, 0.0f, 2.0f) : -1.0f); });
    return true;
}
bool HirariUnifiedEngine::set_sidechain_link(uint32_t sourceId, uint32_t destId,
                                           uint32_t pluginIndex, uint32_t tapPoint,
                                           bool enabled) {
    return set_sidechain_link_internal(sourceId, destId, pluginIndex, tapPoint,
                                       enabled, true);
}

bool HirariUnifiedEngine::set_sidechain_link_internal(uint32_t sourceId, uint32_t destId,
                                                    uint32_t pluginIndex, uint32_t tapPoint,
                                                    bool enabled, bool recordUndo) {
    if (sourceId == destId || tapPoint > 2) return false;
    auto& manager = m_sidechainManager;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    const auto previous = std::find_if(
        m_sidechainRoutes.begin(), m_sidechainRoutes.end(),
        [destId, pluginIndex](const SidechainRoute& route) {
            return route.destinationId == destId && route.pluginIndex == pluginIndex;
        });
    const bool hadPrevious = previous != m_sidechainRoutes.end();
    const uint32_t previousSource = hadPrevious ? previous->sourceId : 0;
    const uint32_t previousTapPoint = hadPrevious ? previous->tapPoint : 2;
    const auto edgeUsedByAnotherPlugin = [this, destId, pluginIndex](uint32_t sourceId) {
        return std::any_of(m_sidechainRoutes.begin(), m_sidechainRoutes.end(),
            [sourceId, destId, pluginIndex](const SidechainRoute& route) {
                return route.sourceId == sourceId && route.destinationId == destId &&
                       route.pluginIndex != pluginIndex;
            });
    };
    if (!enabled) {
        const bool removed = manager.removeLink(destId, pluginIndex);
        if (hadPrevious && !edgeUsedByAnotherPlugin(previousSource))
            m_routingEngine.removeProcessingDependency(previousSource, destId);
        const size_t previousRouteCount = m_sidechainRoutes.size();
        m_sidechainRoutes.erase(
            std::remove_if(m_sidechainRoutes.begin(), m_sidechainRoutes.end(),
                           [destId, pluginIndex](const SidechainRoute& route) {
                               return route.destinationId == destId &&
                                      route.pluginIndex == pluginIndex;
                           }),
            m_sidechainRoutes.end());
        const bool routeRemoved = m_sidechainRoutes.size() != previousRouteCount;
        if (removed || routeRemoved) {
            (void)m_routingEngine.buildGraph();
            mark_project_layout_changed();
        }
        if (!removed && !routeRemoved) return false;
        if (recordUndo) {
            m_undoManager.recordAppliedAction(
                "Sidechain Disconnect",
                [this, previousSource, destId, pluginIndex, previousTapPoint]() {
                    set_sidechain_link_internal(previousSource, destId, pluginIndex,
                                                previousTapPoint, true, false);
                },
                [this, previousSource, destId, pluginIndex, previousTapPoint]() {
                    set_sidechain_link_internal(previousSource, destId, pluginIndex,
                                                previousTapPoint, false, false);
                });
        }
        return true;
    }

    // Sidechain dependencies are directed source -> destination. Adding a
    // link is rejected when the destination can already reach the source;
    // this prevents A->B->A cycles before the native graph is published.
    auto reaches = [this](uint32_t from, uint32_t target) {
        std::vector<uint32_t> pending{from};
        std::vector<uint32_t> visited;
        while (!pending.empty()) {
            const uint32_t node = pending.back();
            pending.pop_back();
            if (node == target) return true;
            if (std::find(visited.begin(), visited.end(), node) != visited.end()) continue;
            visited.push_back(node);
            for (const auto& route : m_sidechainRoutes) {
                if (route.sourceId == node) pending.push_back(route.destinationId);
            }
        }
        return false;
    };
    if (reaches(destId, sourceId)) return false;

    Track* source = nullptr;
    Track* destination = nullptr;
    for (const auto& track : m_tracks) {
        if (track->getId() == sourceId) source = track.get();
        if (track->getId() == destId) destination = track.get();
    }
    const uint32_t blockSize = m_context && m_context->blockSize > 0
        ? std::min(m_context->blockSize, Hirari::Core::Engine::SidechainManager::kMaxBlockSize)
        : 512u;
    if (!source || !destination) return false;
    const bool sourcePrepared = source->canProcess(blockSize);
    const uint32_t sampleRate = static_cast<uint32_t>(std::max(
        1.0, m_sampleRate.load(std::memory_order_acquire)));
    const uint64_t sourceGeneration = std::max<uint64_t>(
        1, m_audioConfigGeneration.load(std::memory_order_acquire));
    const bool newEdgeAlreadyUsed = edgeUsedByAnotherPlugin(sourceId) ||
        (hadPrevious && previousSource == sourceId);
    const bool removedPreviousDependency = hadPrevious && previousSource != sourceId &&
        !edgeUsedByAnotherPlugin(previousSource);
    if (removedPreviousDependency)
        m_routingEngine.removeProcessingDependency(previousSource, destId);
    const auto restorePreviousDependency = [&]() {
        if (removedPreviousDependency) {
            (void)m_routingEngine.addProcessingDependency(previousSource, destId);
            (void)m_routingEngine.buildGraph();
        }
    };
    if (!m_routingEngine.addProcessingDependency(sourceId, destId) ||
        !m_routingEngine.buildGraph()) {
        if (!newEdgeAlreadyUsed)
            m_routingEngine.removeProcessingDependency(sourceId, destId);
        restorePreviousDependency();
        return false;
    }
    if (!sourcePrepared) {
        const bool published = manager.registerSilentLink(
            destId, pluginIndex, sourceId, blockSize, sampleRate,
            sourceGeneration, 1.0f,
            static_cast<Hirari::Core::Engine::SidechainTapPoint>(tapPoint));
        if (!published) {
            if (!newEdgeAlreadyUsed)
                m_routingEngine.removeProcessingDependency(sourceId, destId);
            restorePreviousDependency();
            return false;
        }
    } else {
        auto& sourceBuffer = source->getWorkBuffer(blockSize);
    Hirari::Core::Engine::SidechainLink link;
    link.sourceBufferL = sourceBuffer.getReadPointer(0);
    link.sourceBufferR = sourceBuffer.getNumChannels() > 1
        ? sourceBuffer.getReadPointer(1) : link.sourceBufferL;
    link.sourceTrackId = sourceId;
    link.sourceSampleRate = sampleRate;
    link.sourceGeneration = sourceGeneration;
    link.level = 1.0f;
    link.tapPoint = static_cast<Hirari::Core::Engine::SidechainTapPoint>(tapPoint);
    // Registration copies one prepared block into SidechainManager-owned
    // storage.  Supplying the frame count is part of that ownership contract;
    // without it the manager correctly rejects the link as an unbounded/raw
    // pointer publication.
        link.sourceFrames = blockSize;
        if (!manager.registerLink(destId, pluginIndex, link)) {
            if (!newEdgeAlreadyUsed)
                m_routingEngine.removeProcessingDependency(sourceId, destId);
            restorePreviousDependency();
            return false;
        }
    }
    auto existing = std::find_if(
        m_sidechainRoutes.begin(), m_sidechainRoutes.end(),
        [destId, pluginIndex](const SidechainRoute& route) {
            return route.destinationId == destId && route.pluginIndex == pluginIndex;
        });
    if (existing == m_sidechainRoutes.end()) {
        m_sidechainRoutes.push_back({sourceId, destId, pluginIndex, tapPoint});
    } else {
        existing->sourceId = sourceId;
        existing->tapPoint = tapPoint;
    }
    mark_project_layout_changed();
    if (recordUndo) {
        m_undoManager.recordAppliedAction(
            "Sidechain Connect",
            [this, hadPrevious, previousSource, destId, pluginIndex, previousTapPoint]() {
                if (hadPrevious) {
                    set_sidechain_link_internal(previousSource, destId, pluginIndex,
                                                previousTapPoint, true, false);
                } else {
                    set_sidechain_link_internal(0, destId, pluginIndex, 0, false, false);
                }
            },
            [this, sourceId, destId, pluginIndex, tapPoint]() {
                set_sidechain_link_internal(sourceId, destId, pluginIndex, tapPoint,
                                            true, false);
            });
    }
    return true;
}

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

    Region left = before;
    Region right = before;
    left.len = splitOffset;
    right.start = before.start + splitOffset;
    right.len = before.len - splitOffset;
    // Clip-edge fades belong to the outer edges after a split. Copying both
    // fades to both halves creates two new dips at the edit point.
    left.fadeInSamples = std::min(before.fadeInSamples, left.len);
    left.fadeOutSamples = 0;
    right.fadeInSamples = 0;
    right.fadeOutSamples = std::min(before.fadeOutSamples, right.len);
    const uint64_t sourceLength = before.sourceLength == 0 ? before.len : before.sourceLength;
    if (sourceLength <= 1) return 0;
    const auto sourcePositionAt = [&before, sourceLength](uint64_t timelineSample) {
        const auto& markers = before.warpMarkers;
        if (markers.size() < 2) {
            return std::clamp(static_cast<double>(timelineSample) * before.sourceFramesPerTimelineFrame(),
                              0.0, static_cast<double>(sourceLength));
        }
        auto upper = std::upper_bound(
            markers.begin(), markers.end(), timelineSample,
            [](uint64_t sample, const Region::WarpMarker& marker) {
                return sample < marker.timelineSample;
            });
        const Region::WarpMarker* left = nullptr;
        const Region::WarpMarker* right = nullptr;
        if (upper == markers.begin()) {
            left = &markers[0];
            right = &markers[1];
        } else if (upper == markers.end()) {
            left = &markers[markers.size() - 2];
            right = &markers.back();
        } else {
            left = &*std::prev(upper);
            right = &*upper;
        }
        const uint64_t timelineDelta = right->timelineSample - left->timelineSample;
        if (timelineDelta == 0) return static_cast<double>(left->sourceSample);
        const double sourcePerTimeline = static_cast<double>(
            right->sourceSample - left->sourceSample) / static_cast<double>(timelineDelta);
        return std::clamp(
            static_cast<double>(left->sourceSample) +
                (static_cast<double>(timelineSample) - static_cast<double>(left->timelineSample)) *
                    sourcePerTimeline,
            0.0, static_cast<double>(sourceLength));
    };
    const double mappedSplit = sourcePositionAt(splitOffset);
    if (!std::isfinite(mappedSplit)) return 0;
    const uint64_t sourceSplit = static_cast<uint64_t>(std::clamp(
        std::round(mappedSplit), 1.0, static_cast<double>(sourceLength - 1)));
    left.sourceLength = sourceSplit;
    right.sourceLength = sourceLength - sourceSplit;
    // Preserve the piecewise source/timeline mapping on both halves. The
    // split boundary is evaluated through the same map as playback, then
    // becomes the right edge of the left map and the origin of the right map.
    if (before.warpMarkers.size() >= 2) {
        const Region::WarpMarker leftStart{
            static_cast<uint64_t>(std::clamp(std::round(sourcePositionAt(0)), 0.0,
                                             static_cast<double>(sourceSplit))),
            0, false};
        const Region::WarpMarker leftBoundary{sourceSplit, splitOffset, false};
        const Region::WarpMarker rightBoundary{0, 0, false};
        left.warpMarkers.clear();
        right.warpMarkers.clear();
        left.warpMarkers.reserve(before.warpMarkers.size() + 1);
        right.warpMarkers.reserve(before.warpMarkers.size() + 1);
        const double mappedEnd = sourcePositionAt(before.len);
        if (!std::isfinite(mappedEnd)) return 0;
        const uint64_t rightEndSource = static_cast<uint64_t>(std::clamp(
            std::round(mappedEnd) - static_cast<double>(sourceSplit), 0.0,
            static_cast<double>(right.sourceLength)));
        for (const auto& marker : before.warpMarkers) {
            if (marker.timelineSample < splitOffset &&
                marker.sourceSample > leftStart.sourceSample &&
                marker.sourceSample < sourceSplit) {
                left.warpMarkers.push_back(marker);
            }
            else if (marker.timelineSample == splitOffset) left.warpMarkers.push_back(leftBoundary);
            if (marker.timelineSample > splitOffset && marker.sourceSample > sourceSplit &&
                marker.sourceSample - sourceSplit < rightEndSource) {
                right.warpMarkers.push_back({marker.sourceSample - sourceSplit,
                                             marker.timelineSample - splitOffset,
                                             marker.transient});
            }
        }
        if (left.warpMarkers.empty() ||
            left.warpMarkers.back().timelineSample != splitOffset) {
            left.warpMarkers.push_back(leftBoundary);
        } else {
            left.warpMarkers.back() = leftBoundary;
        }
        if (right.warpMarkers.empty() || right.warpMarkers.front().timelineSample != 0) {
            right.warpMarkers.insert(right.warpMarkers.begin(), rightBoundary);
        } else {
            right.warpMarkers.front() = rightBoundary;
        }
        if (left.warpMarkers.front().timelineSample != 0) {
            left.warpMarkers.insert(left.warpMarkers.begin(), leftStart);
        } else {
            left.warpMarkers.front() = leftStart;
        }
        const Region::WarpMarker rightEnd{rightEndSource, right.len, false};
        if (right.warpMarkers.back().timelineSample != right.len) {
            right.warpMarkers.push_back(rightEnd);
        } else {
            right.warpMarkers.back() = rightEnd;
        }
    } else {
        left.warpMarkers.clear();
        right.warpMarkers.clear();
    }
    // Pitch/formant note edits use region-relative seconds. Split each segment
    // at the same timeline boundary and move the right half back to time zero;
    // copying the original segment vector to both clips applies stale edits to
    // unrelated audio after a split.
    const double splitSeconds = static_cast<double>(splitOffset) /
        std::max(1.0, m_sampleRate.load(std::memory_order_acquire));
    const double regionSeconds = static_cast<double>(before.len) /
        std::max(1.0, m_sampleRate.load(std::memory_order_acquire));
    left.audioNoteSegments.clear();
    right.audioNoteSegments.clear();
    const auto appendSegmentWindow = [](const ::hirari::editing::AudioNoteSegment& source,
                                        double start, double end, double timeShift,
                                        auto& destination) {
        if (!(end > start)) return;
        auto segment = source;
        segment.startSeconds = start - timeShift;
        segment.endSeconds = end - timeShift;
        if (!source.anchors.empty()) {
            std::vector<::hirari::editing::AudioNoteAnchor> anchors;
            anchors.reserve(source.anchors.size() + 2);
            const auto boundaryAnchor = [&source](double position) {
                return ::hirari::editing::AudioNoteAnchor{
                    position,
                    source.pitchAt(position) - source.pitchOffsetCents,
                    source.formantAt(position) - source.formantOffsetCents};
            };
            anchors.push_back(boundaryAnchor(start));
            for (const auto& anchor : source.anchors) {
                if (anchor.positionSeconds > start && anchor.positionSeconds < end) {
                    auto shifted = anchor;
                    anchors.push_back(shifted);
                }
            }
            auto endAnchor = boundaryAnchor(end);
            endAnchor.positionSeconds -= timeShift;
            anchors.push_back(endAnchor);
            for (auto& anchor : anchors) anchor.positionSeconds -= timeShift;
            segment.anchors = std::move(anchors);
        }
        segment.pitchRatioIntegralPrefix.clear();
        segment.pitchRatioCorrectionBeforeSeconds = 0.0;
        destination.push_back(std::move(segment));
    };
    for (const auto& segment : before.audioNoteSegments) {
        appendSegmentWindow(segment, segment.startSeconds,
                            std::min(segment.endSeconds, splitSeconds), 0.0,
                            left.audioNoteSegments);
        appendSegmentWindow(segment, std::max(segment.startSeconds, splitSeconds),
                            std::min(segment.endSeconds, regionSeconds), splitSeconds,
                            right.audioNoteSegments);
    }
    const auto splitLocalRanges = [splitOffset, regionLength = before.len](
        const auto& sourceRanges, auto& leftRanges, auto& rightRanges) {
        leftRanges.clear();
        rightRanges.clear();
        leftRanges.reserve(sourceRanges.size());
        rightRanges.reserve(sourceRanges.size());
        for (const auto& sourceRange : sourceRanges) {
            const uint64_t sourceStart = sourceRange.start;
            const uint64_t sourceEnd = std::min(sourceRange.end, regionLength);
            if (sourceStart >= sourceEnd) continue;
            const uint64_t leftEnd = std::min(sourceEnd, splitOffset);
            if (sourceStart < leftEnd) {
                auto range = sourceRange;
                range.start = sourceStart;
                range.end = leftEnd;
                const uint64_t length = range.end - range.start;
                range.fadeIn = sourceStart == sourceRange.start
                    ? std::min(sourceRange.fadeIn, length) : 0;
                range.fadeOut = leftEnd == sourceRange.end
                    ? std::min(sourceRange.fadeOut, length) : 0;
                leftRanges.push_back(std::move(range));
            }
            const uint64_t rightStart = std::max(sourceStart, splitOffset);
            if (rightStart < sourceEnd) {
                auto range = sourceRange;
                range.start = rightStart - splitOffset;
                range.end = sourceEnd - splitOffset;
                const uint64_t length = range.end - range.start;
                range.fadeIn = rightStart == sourceRange.start
                    ? std::min(sourceRange.fadeIn, length) : 0;
                range.fadeOut = sourceEnd == sourceRange.end
                    ? std::min(sourceRange.fadeOut, length) : 0;
                rightRanges.push_back(std::move(range));
            }
        }
    };
    splitLocalRanges(before.rangeEdits, left.rangeEdits, right.rangeEdits);
    splitLocalRanges(before.compRanges, left.compRanges, right.compRanges);
    if (right.sourceLength == 0) return 0;
    if (before.reverse) {
        const uint64_t leftSourceOffset = sourceLength - sourceSplit;
        if (before.sourceOffset > UINT64_MAX - leftSourceOffset) return 0;
        left.sourceOffset = before.sourceOffset + leftSourceOffset;
    } else {
        if (before.sourceOffset > UINT64_MAX - sourceSplit) return 0;
        right.sourceOffset = before.sourceOffset + sourceSplit;
    }
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
        std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
        m_peakCache.erase(rid);
        m_peakCache.erase(right.id);
    };
    const uint32_t rightId = right.id;
    const auto restore = [this, target, rid, rightId, before]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->removeRegion(rid);
        target->removeRegion(rightId);
        target->addRegion(before);
        std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
        m_peakCache.erase(rid);
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
            if (region.id != rid || region.len == 0 || region.loopCount == 0 ||
                region.len > UINT64_MAX / region.loopCount ||
                startSample > UINT64_MAX - region.len * region.loopCount)
                continue;
            const uint64_t sourceTrim = region.sourceOffset >= region.baseSourceOffset
                ? region.sourceOffset - region.baseSourceOffset : 0;
            const long double timelineTrim = std::ceil(
                static_cast<long double>(sourceTrim) / region.sourceFramesPerTimelineFrame());
            if (!std::isfinite(static_cast<double>(timelineTrim)) || timelineTrim < 0.0L ||
                timelineTrim > static_cast<long double>(startSample)) continue;
            target = track;
            duplicate = region;
            duplicate.id = allocateRegionIdLocked();
            if (duplicate.id == 0) return 0;
            duplicate.start = startSample;
            duplicate.baseStart = startSample - static_cast<uint64_t>(timelineTrim);
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
        std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
        m_peakCache.erase(duplicateId);
    };
    m_undoManager.performAction(
        "Duplicate Region", remove, apply);
    return duplicateId;
}
