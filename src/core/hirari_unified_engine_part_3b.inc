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
    hirari_macro_control_set_value(m_macroControl, macroIndex, value);
}
bool HirariUnifiedEngine::bind_midi_cc_to_macro(uint8_t channel, uint8_t cc,
                                               uint32_t macroIndex, float minimum,
                                               float maximum, float curve,
                                               bool pickup) noexcept {
    if (macroIndex >= kMaxMacros || channel > 16 || cc > 127 ||
        !std::isfinite(minimum) || !std::isfinite(maximum) || minimum > maximum ||
        !std::isfinite(curve) || curve < -1.0f || curve > 1.0f) {
        return false;
    }
    const void* targetState = m_macroControl;
    if (targetState == nullptr) return false;
    m_midiLearnManager.addMappingAdvanced(
        cc, channel, targetState, macroIndex, minimum, maximum, curve, pickup);
    return true;
}
bool HirariUnifiedEngine::bind_midi_cc14_to_macro(uint8_t channel, uint16_t controller,
                                                 uint32_t macroIndex, float minimum,
                                                 float maximum, float curve,
                                                 bool pickup) noexcept {
    if (macroIndex >= kMaxMacros || channel > 16 || controller > 16383 ||
        !std::isfinite(minimum) || !std::isfinite(maximum) || minimum > maximum ||
        !std::isfinite(curve) || curve < -1.0f || curve > 1.0f) {
        return false;
    }
    const void* targetState = m_macroControl;
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
    };
    hirari_routing_edit_host_install(&ops);
}

namespace {
[[maybe_unused]] const bool kRoutingEditHostInstalled = [] {
    HirariUnifiedEngine::installRoutingEditHost();
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
    static_assert(sizeof(Region::WarpMarker) == sizeof(HirariWarpMarker));
    std::vector<Region::WarpMarker> leftMarkers(before.warpMarkers.size() + 2);
    std::vector<Region::WarpMarker> rightMarkers(before.warpMarkers.size() + 2);
    size_t leftMarkerCount = 0;
    size_t rightMarkerCount = 0;
    uint64_t sourceSplit = 0;
    if (!hirari_region_split_warp_markers(
            reinterpret_cast<const HirariWarpMarker*>(before.warpMarkers.data()),
            before.warpMarkers.size(), sourceLength, before.len, splitOffset,
            before.sourceFramesPerTimelineFrame(),
            reinterpret_cast<HirariWarpMarker*>(leftMarkers.data()), leftMarkers.size(),
            &leftMarkerCount, reinterpret_cast<HirariWarpMarker*>(rightMarkers.data()),
            rightMarkers.size(), &rightMarkerCount, &sourceSplit)) return 0;
    leftMarkers.resize(leftMarkerCount);
    rightMarkers.resize(rightMarkerCount);
    left.sourceLength = sourceSplit;
    right.sourceLength = sourceLength - sourceSplit;
    // Preserve the piecewise source/timeline mapping on both halves. The
    // split boundary is evaluated through the same map as playback, then
    // becomes the right edge of the left map and the origin of the right map.
    left.warpMarkers = std::move(leftMarkers);
    right.warpMarkers = std::move(rightMarkers);
    // Pitch/formant note edits use region-relative seconds. Split each segment
    // at the same timeline boundary and move the right half back to time zero;
    // copying the original segment vector to both clips applies stale edits to
    // unrelated audio after a split.
    const double splitSeconds = static_cast<double>(splitOffset) /
        std::max(1.0, m_sampleRate.load(std::memory_order_acquire));
    const double regionSeconds = static_cast<double>(before.len) /
        std::max(1.0, m_sampleRate.load(std::memory_order_acquire));
    std::vector<::hirari::editing::AudioNoteSegment> beforeSegments;
    if (!before.noteEdits.copy(beforeSegments)) return false;
    left.noteEdits.detach();
    right.noteEdits.detach();
    std::vector<::hirari::editing::AudioNoteSegment> leftSegments;
    std::vector<::hirari::editing::AudioNoteSegment> rightSegments;
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
    for (const auto& segment : beforeSegments) {
        appendSegmentWindow(segment, segment.startSeconds,
                            std::min(segment.endSeconds, splitSeconds), 0.0,
                            leftSegments);
        appendSegmentWindow(segment, std::max(segment.startSeconds, splitSeconds),
                            std::min(segment.endSeconds, regionSeconds), splitSeconds,
                            rightSegments);
    }
    if (!left.noteEdits.assignRepaired(leftSegments) ||
        !right.noteEdits.assignRepaired(rightSegments)) {
        return false;
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
