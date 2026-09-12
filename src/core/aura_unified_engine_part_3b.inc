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
bool AuraUnifiedEngine::get_plugin_bypass(uint32_t tid, uint32_t pluginIndex) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track->getId() == tid) return track->getPluginBypass(pluginIndex);
    }
    return false;
}
bool AuraUnifiedEngine::move_plugin(uint32_t tid, uint32_t fromIndex, uint32_t toIndex) {
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
bool AuraUnifiedEngine::set_phase_invert(uint32_t tid, bool inverted) {
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
bool AuraUnifiedEngine::set_track_delay_samples(uint32_t tid, uint32_t samples) {
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
uint32_t AuraUnifiedEngine::get_track_delay_samples(uint32_t tid) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid) return track->getTrackDelaySamples();
    }
    return 0;
}
bool AuraUnifiedEngine::set_track_armed(uint32_t tid, bool armed) {
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

bool AuraUnifiedEngine::set_track_input_monitor(uint32_t tid, bool enabled) {
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
    m_inputMonitorTrackId.store(tid, std::memory_order_release);
    return true;
}

void AuraUnifiedEngine::set_macro_value(uint32_t macroIndex, float value) {
    if (!std::isfinite(value)) return;
    m_macroControlManager.setMacroValue(macroIndex, value);
}
bool AuraUnifiedEngine::bind_midi_cc_to_macro(uint8_t channel, uint8_t cc,
                                               uint32_t macroIndex, float minimum,
                                               float maximum, float curve,
                                               bool pickup) noexcept {
    if (macroIndex >= MacroControlManager::kMaxMacros || channel > 16 || cc > 127 ||
        !std::isfinite(minimum) || !std::isfinite(maximum) || minimum > maximum ||
        !std::isfinite(curve) || curve < -1.0f || curve > 1.0f) {
        return false;
    }
    auto* target = m_macroControlManager.targetPointer(macroIndex);
    if (target == nullptr) return false;
    m_midiLearnManager.addMappingAdvanced(
        cc, channel, target, minimum, maximum, curve, pickup);
    return true;
}
bool AuraUnifiedEngine::bind_midi_cc14_to_macro(uint8_t channel, uint16_t controller,
                                                 uint32_t macroIndex, float minimum,
                                                 float maximum, float curve,
                                                 bool pickup) noexcept {
    if (macroIndex >= MacroControlManager::kMaxMacros || channel > 16 || controller > 16383 ||
        !std::isfinite(minimum) || !std::isfinite(maximum) || minimum > maximum ||
        !std::isfinite(curve) || curve < -1.0f || curve > 1.0f) {
        return false;
    }
    auto* target = m_macroControlManager.targetPointer(macroIndex);
    if (target == nullptr) return false;
    m_midiLearnManager.addMapping14Bit(
        controller, channel, target, minimum, maximum, curve, pickup);
    return true;
}
void AuraUnifiedEngine::handle_midi_cc(uint8_t channel, uint8_t cc,
                                       uint8_t value) noexcept {
    m_midiLearnManager.handleMidiCC(channel, cc, value);
}

void AuraUnifiedEngine::handle_midi_cc14(uint8_t channel, uint16_t controller,
                                         uint16_t value) noexcept {
    m_midiLearnManager.handleMidiCC14(channel, controller, value);
}
void AuraUnifiedEngine::set_preview_synth_engine(uint32_t engine) {
    m_previewSynthEngine.store(std::min(engine, 2u), std::memory_order_release);
}
bool AuraUnifiedEngine::set_route(uint32_t sourceId, uint32_t destId, bool enabled) {
    if (sourceId >= Aura::Core::Engine::RoutingEngine::kMaxNodes ||
        destId >= Aura::Core::Engine::RoutingEngine::kMaxNodes || sourceId == destId) return false;
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
bool AuraUnifiedEngine::set_route_gain(uint32_t sourceId, uint32_t destId, float gain, bool enabled) {
    if (sourceId >= Aura::Core::Engine::RoutingEngine::kMaxNodes ||
        destId >= Aura::Core::Engine::RoutingEngine::kMaxNodes || sourceId == destId ||
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
bool AuraUnifiedEngine::set_feedback_route(uint32_t sourceId, uint32_t destId,
                                           float gain, bool enabled) {
    if (sourceId >= Aura::Core::Engine::RoutingEngine::kMaxNodes ||
        destId >= Aura::Core::Engine::RoutingEngine::kMaxNodes || sourceId == destId ||
        !std::isfinite(gain)) return false;
    auto& routing = m_routingEngine;
    const float beforeGain = routing.feedbackConnectionGain(sourceId, destId);
    const bool changed = enabled
        ? routing.addFeedbackConnection(sourceId, destId, gain)
        : (beforeGain >= 0.0f ? (routing.removeFeedbackConnection(sourceId, destId), true) : false);
    if (!changed) return false;
    const auto apply = [&routing, sourceId, destId](float nextGain) {
        if (nextGain < 0.0f) routing.removeFeedbackConnection(sourceId, destId);
        else routing.addFeedbackConnection(sourceId, destId, nextGain);
    };
    m_undoManager.recordAppliedAction(
        "Feedback Route", [apply, beforeGain] { apply(beforeGain); },
        [apply, gain, enabled] { apply(enabled ? std::clamp(gain, 0.0f, 2.0f) : -1.0f); });
    return true;
}
bool AuraUnifiedEngine::set_sidechain_link(uint32_t sourceId, uint32_t destId,
                                           uint32_t pluginIndex, uint32_t tapPoint,
                                           bool enabled) {
    return set_sidechain_link_internal(sourceId, destId, pluginIndex, tapPoint,
                                       enabled, true);
}

bool AuraUnifiedEngine::set_sidechain_link_internal(uint32_t sourceId, uint32_t destId,
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
    if (!enabled) {
        const bool removed = manager.removeLink(destId, pluginIndex);
        m_sidechainRoutes.erase(
            std::remove_if(m_sidechainRoutes.begin(), m_sidechainRoutes.end(),
                           [destId, pluginIndex](const SidechainRoute& route) {
                               return route.destinationId == destId &&
                                      route.pluginIndex == pluginIndex;
                           }),
            m_sidechainRoutes.end());
        if (!removed) return false;
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
        ? std::min(m_context->blockSize, Aura::Core::Engine::SidechainManager::kMaxBlockSize)
        : 512u;
    if (!source || !destination) return false;
    const bool sourcePrepared = source->canProcess(blockSize);
    const uint32_t sampleRate = static_cast<uint32_t>(std::max(
        1.0, m_sampleRate.load(std::memory_order_acquire)));
    const uint64_t sourceGeneration = std::max<uint64_t>(
        1, m_audioConfigGeneration.load(std::memory_order_acquire));
    if (!sourcePrepared) {
        const bool published = manager.registerSilentLink(
            destId, pluginIndex, sourceId, blockSize, sampleRate,
            sourceGeneration, 1.0f,
            static_cast<Aura::Core::Engine::SidechainTapPoint>(tapPoint));
        if (!published) return false;
    } else {
        auto& sourceBuffer = source->getWorkBuffer(blockSize);
    Aura::Core::Engine::SidechainLink link;
    link.sourceBufferL = sourceBuffer.getReadPointer(0);
    link.sourceBufferR = sourceBuffer.getNumChannels() > 1
        ? sourceBuffer.getReadPointer(1) : link.sourceBufferL;
    link.sourceTrackId = sourceId;
    link.sourceSampleRate = sampleRate;
    link.sourceGeneration = sourceGeneration;
    link.level = 1.0f;
    link.tapPoint = static_cast<Aura::Core::Engine::SidechainTapPoint>(tapPoint);
    // Registration copies one prepared block into SidechainManager-owned
    // storage.  Supplying the frame count is part of that ownership contract;
    // without it the manager correctly rejects the link as an unbounded/raw
    // pointer publication.
    link.sourceFrames = blockSize;
        if (!manager.registerLink(destId, pluginIndex, link)) return false;
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

bool AuraUnifiedEngine::has_sidechain_link(uint32_t sourceId, uint32_t destId,
                                           uint32_t pluginIndex) const {
    return m_sidechainManager.hasLink(
        destId, pluginIndex, sourceId);
}
bool AuraUnifiedEngine::split_region(uint32_t tid, uint32_t rid, double splitBeat) {
    // The bridge passes an absolute sample position as a double for ABI
    // compatibility. Reject fractional positions instead of truncating them
    // and silently moving an edit by part of a sample.
    if (!std::isfinite(splitBeat) || splitBeat < 0.0 || std::floor(splitBeat) != splitBeat) return false;
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
    if (!target || before.len == 0 || before.start > UINT64_MAX - before.len) return false;
    const double end = static_cast<double>(before.start) + static_cast<double>(before.len);
    if (splitBeat <= static_cast<double>(before.start) || splitBeat >= end) return false;
    const uint64_t splitOffset = static_cast<uint64_t>(splitBeat - static_cast<double>(before.start));
    if (splitOffset == 0 || splitOffset >= before.len || before.sourceOffset > UINT64_MAX - splitOffset)
        return false;

    Region left = before;
    Region right = before;
    left.len = splitOffset;
    right.start = before.start + splitOffset;
    right.len = before.len - splitOffset;
    right.sourceOffset += splitOffset;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        right.id = allocateRegionIdLocked();
    }
    if (right.id == 0) return false;

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
    return true;
}

uint32_t AuraUnifiedEngine::duplicate_region(uint32_t tid, uint32_t rid, uint64_t startSample) {
    std::shared_ptr<Track> target;
    Region duplicate;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
        if (track->getId() != tid) continue;
        for (const auto& region : track->getRegions()) {
            if (region.id != rid || region.len == 0 || startSample > UINT64_MAX - region.len)
                continue;
            target = track;
            duplicate = region;
            duplicate.id = allocateRegionIdLocked();
            if (duplicate.id == 0) return 0;
            duplicate.start = startSample;
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
