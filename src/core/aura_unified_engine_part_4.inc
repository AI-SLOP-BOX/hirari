bool AuraUnifiedEngine::move_region(uint32_t tid, uint32_t rid, double startBeat) {
    if (!std::isfinite(startBeat) || startBeat < 0.0) return false;
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
    if (!target) return false;
    if (startBeat > static_cast<double>(UINT64_MAX)) return false;

    Region moved = before;
    moved.start = static_cast<uint64_t>(startBeat);
    const auto apply = [this, target, moved, rid]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->removeRegion(rid);
        target->addRegion(moved);
        std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
        m_peakCache.erase(rid);
    };
    const auto undo = [this, target, before, rid]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->removeRegion(rid);
        target->addRegion(before);
        std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
        m_peakCache.erase(rid);
    };
    m_undoManager.performAction("Move Region", undo, apply);
    return true;
}

bool AuraUnifiedEngine::remove_region(uint32_t tid, uint32_t rid) {
    if (tid == 0 || rid == 0) return false;
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
    if (!target) return false;

    const auto remove = [this, target, rid]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->removeRegion(rid);
        std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
        m_peakCache.erase(rid);
    };
    const auto restore = [this, target, before, rid]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->removeRegion(rid);
        target->addRegion(before);
        std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
        m_peakCache.erase(rid);
    };
    m_undoManager.performAction("Remove Region", restore, remove);
    return true;
}

bool AuraUnifiedEngine::set_region_gain(uint32_t tid, uint32_t rid, float gain) {
    if (!std::isfinite(gain)) return false;
    std::shared_ptr<Track> target;
    float before = 1.0f;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() != tid) continue;
            for (const auto& region : track->getRegions()) {
                if (region.id == rid) {
                    target = track;
                    before = region.clipGain;
                    break;
                }
            }
            break;
        }
    }
    if (!target) return false;
    const float bounded = std::clamp(gain, 0.0f, 2.0f);
    auto apply = [target, rid](float value) { target->setRegionGain(rid, value); };
    m_undoManager.performAction(
        "Region Gain", [apply, before] { apply(before); }, [apply, bounded] { apply(bounded); });
    return true;
}

bool AuraUnifiedEngine::set_region_muted(uint32_t tid, uint32_t rid, bool muted) {
    std::shared_ptr<Track> target;
    bool before = false;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() != tid) continue;
            for (const auto& region : track->getRegions()) {
                if (region.id == rid) {
                    target = track;
                    before = region.muted;
                    break;
                }
            }
            break;
        }
    }
    if (!target) return false;
    auto apply = [target, rid](bool value) { target->setRegionMuted(rid, value); };
    m_undoManager.performAction(
        "Mute Region", [apply, before] { apply(before); }, [apply, muted] { apply(muted); });
    return true;
}

bool AuraUnifiedEngine::set_region_fades(uint32_t tid, uint32_t rid, float fadeIn, float fadeOut) {
    if (!std::isfinite(fadeIn) || !std::isfinite(fadeOut)) return false;
    std::shared_ptr<Track> target;
    float beforeIn = 0.0f;
    float beforeOut = 0.0f;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() != tid) continue;
            for (const auto& region : track->getRegions()) {
                if (region.id == rid) {
                    target = track;
                    if (region.len > 0) {
                        beforeIn = static_cast<float>(region.fadeInSamples) / region.len;
                        beforeOut = static_cast<float>(region.fadeOutSamples) / region.len;
                    }
                    break;
                }
            }
            break;
        }
    }
    if (!target) return false;
    const float boundedIn = std::clamp(fadeIn, 0.0f, 1.0f);
    const float boundedOut = std::clamp(fadeOut, 0.0f, 1.0f);
    auto apply = [target, rid](float in, float out) { target->setRegionFades(rid, in, out); };
    m_undoManager.performAction(
        "Region Fades", [apply, beforeIn, beforeOut] { apply(beforeIn, beforeOut); },
        [apply, boundedIn, boundedOut] { apply(boundedIn, boundedOut); });
    return true;
}

bool AuraUnifiedEngine::set_region_range_edit(uint32_t tid, uint32_t rid, uint64_t start,
                                              uint64_t end, float gain, uint64_t fadeIn,
                                              uint64_t fadeOut) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track->getId() != tid) continue;
        const auto before = track->regionRangeEdits(rid);
        if (!track->setRegionRangeEdit(rid, start, end, gain, fadeIn, fadeOut)) return false;
        auto after = track->regionRangeEdits(rid);
        m_undoManager.performAction("Region Range Edit",
            [track, rid, before] { track->replaceRegionRangeEdits(rid, before); },
            [track, rid, after] { track->replaceRegionRangeEdits(rid, after); });
        return true;
    }
    return false;
}

bool AuraUnifiedEngine::clear_region_range_edit(uint32_t tid, uint32_t rid, uint64_t start,
                                                uint64_t end) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track->getId() != tid) continue;
        const auto before = track->regionRangeEdits(rid);
        if (!track->clearRegionRangeEdit(rid, start, end)) return false;
        auto after = track->regionRangeEdits(rid);
        m_undoManager.performAction("Clear Region Range Edit",
            [track, rid, before] { track->replaceRegionRangeEdits(rid, before); },
            [track, rid, after] { track->replaceRegionRangeEdits(rid, after); });
        return true;
    }
    return false;
}

bool AuraUnifiedEngine::clear_region_range_edits(uint32_t tid, uint32_t rid) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track->getId() != tid) continue;
        const auto before = track->regionRangeEdits(rid);
        if (before.empty() || !track->clearRegionRangeEdits(rid)) return false;
        m_undoManager.performAction("Clear Region Range Edits",
            [track, rid, before] { track->replaceRegionRangeEdits(rid, before); },
            [track, rid] { track->clearRegionRangeEdits(rid); });
        return true;
    }
    return false;
}

bool AuraUnifiedEngine::set_region_reverse(uint32_t tid, uint32_t rid, bool reverse) {
    std::shared_ptr<Track> target;
    bool before = false;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() != tid) continue;
            for (const auto& region : track->getRegions()) {
                if (region.id == rid) {
                    target = track;
                    before = region.reverse;
                    break;
                }
            }
            break;
        }
    }
    if (!target) return false;
    auto apply = [target, rid](bool value) { target->setRegionReverse(rid, value); };
    m_undoManager.performAction(
        "Reverse Region", [apply, before] { apply(before); }, [apply, reverse] { apply(reverse); });
    return true;
}

bool AuraUnifiedEngine::set_region_trim(uint32_t tid, uint32_t rid, float startNorm, float endNorm) {
    if (!std::isfinite(startNorm) || !std::isfinite(endNorm)) return false;
    const float boundedStart = std::clamp(startNorm, 0.0f, 1.0f);
    const float boundedEnd = std::clamp(endNorm, 0.0f, 1.0f);
    if (boundedEnd <= boundedStart) return false;
    std::shared_ptr<Track> target;
    float beforeStart = 0.0f;
    float beforeEnd = 1.0f;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() != tid) continue;
            for (const auto& region : track->getRegions()) {
                if (region.id == rid && region.baseLength > 0) {
                    target = track;
                    beforeStart = static_cast<float>(region.sourceOffset - region.baseSourceOffset) /
                                  static_cast<float>(region.baseLength);
                    beforeEnd = static_cast<float>(region.sourceOffset - region.baseSourceOffset + region.len) /
                                static_cast<float>(region.baseLength);
                    break;
                }
            }
            break;
        }
    }
    if (!target) return false;
    auto apply = [target, rid](float start, float end) { target->setRegionTrim(rid, start, end); };
    m_undoManager.performAction(
        "Trim Region", [apply, beforeStart, beforeEnd] { apply(beforeStart, beforeEnd); },
        [apply, boundedStart, boundedEnd] { apply(boundedStart, boundedEnd); });
    return true;
}

bool AuraUnifiedEngine::set_region_warp_ratio(uint32_t tid, uint32_t rid, double ratio) {
    if (!std::isfinite(ratio) || ratio < 0.5 || ratio > 2.0) return false;
    std::shared_ptr<Track> target;
    double before = 1.0;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() != tid) continue;
            const auto current = track->getRegionWarpRatio(rid);
            if (!current) return false;
            target = track;
            before = *current;
            break;
        }
    }
    if (!target) return false;
    auto apply = [target, rid](double value) { target->setRegionWarpRatio(rid, value); };
    m_undoManager.performAction(
        "Flex Warp Ratio", [apply, before] { apply(before); }, [apply, ratio] { apply(ratio); });
    return true;
}

bool AuraUnifiedEngine::set_region_pitch_semitones(uint32_t tid, uint32_t rid, float semitones) {
    if (!std::isfinite(semitones) || semitones < -24.0f || semitones > 24.0f) return false;
    std::shared_ptr<Track> target;
    float before = 0.0f;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() != tid) continue;
            const auto current = track->getRegionPitchSemitones(rid);
            if (!current) return false;
            target = track;
            before = *current;
            break;
        }
    }
    if (!target) return false;
    auto apply = [target, rid](float value) { target->setRegionPitchSemitones(rid, value); };
    m_undoManager.performAction(
        "Flex Pitch", [apply, before] { apply(before); }, [apply, semitones] { apply(semitones); });
    return true;
}

bool AuraUnifiedEngine::set_region_audio_note_segment(
    uint32_t tid, uint32_t rid, double startSeconds, double endSeconds,
    double pitchOffsetCents, double formantOffsetCents) {
    if (!std::isfinite(startSeconds) || !std::isfinite(endSeconds) ||
        endSeconds <= startSeconds || endSeconds - startSeconds > 24.0 * 60.0 ||
        !std::isfinite(pitchOffsetCents) || !std::isfinite(formantOffsetCents) ||
        std::abs(pitchOffsetCents) > 4800.0 || std::abs(formantOffsetCents) > 2400.0) {
        return false;
    }
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() == tid) { target = track; break; }
        }
    }
    if (!target) return false;
    ::aura::editing::AudioNoteSegment segment;
    segment.startSeconds = startSeconds;
    segment.endSeconds = endSeconds;
    segment.setPitchOffset(pitchOffsetCents);
    segment.setFormantOffset(formantOffsetCents);
    return target->upsertAudioNoteSegment(rid, std::move(segment));
}

bool AuraUnifiedEngine::set_region_audio_note_anchor(
    uint32_t tid, uint32_t rid, double segmentStartSeconds,
    double positionSeconds, double pitchCents, double formantCents) {
    if (!std::isfinite(segmentStartSeconds) || segmentStartSeconds < 0.0 ||
        !std::isfinite(positionSeconds) || positionSeconds < 0.0 ||
        !std::isfinite(pitchCents) || std::abs(pitchCents) > 4800.0 ||
        !std::isfinite(formantCents) || std::abs(formantCents) > 2400.0) {
        return false;
    }
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() == tid) { target = track; break; }
        }
    }
    if (!target) return false;
    ::aura::editing::AudioNoteAnchor anchor;
    anchor.positionSeconds = positionSeconds;
    anchor.pitchCents = pitchCents;
    anchor.formantCents = formantCents;
    return target->upsertAudioNoteAnchor(rid, segmentStartSeconds, anchor);
}

bool AuraUnifiedEngine::clear_region_audio_note_segments(uint32_t tid, uint32_t rid) {
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() == tid) { target = track; break; }
        }
    }
    return target && target->clearAudioNoteSegments(rid);
}

bool AuraUnifiedEngine::warp_region_audio_note_segment(
    uint32_t tid, uint32_t rid, double segmentStartSeconds,
    double newStartSeconds, double newEndSeconds) {
    if (!std::isfinite(segmentStartSeconds) || segmentStartSeconds < 0.0 ||
        !std::isfinite(newStartSeconds) || newStartSeconds < 0.0 ||
        !std::isfinite(newEndSeconds) || newEndSeconds <= newStartSeconds ||
        newEndSeconds - newStartSeconds > 24.0 * 60.0) return false;
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks)
            if (track->getId() == tid) { target = track; break; }
    }
    return target && target->warpAudioNoteSegment(rid, segmentStartSeconds,
                                                   newStartSeconds, newEndSeconds);
}

bool AuraUnifiedEngine::remove_region_audio_note_segment(uint32_t tid, uint32_t rid,
                                                          double segmentStartSeconds) {
    if (!std::isfinite(segmentStartSeconds) || segmentStartSeconds < 0.0) return false;
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks)
            if (track->getId() == tid) { target = track; break; }
    }
    return target && target->removeAudioNoteSegment(rid, segmentStartSeconds);
}

bool AuraUnifiedEngine::analyze_region_audio_note_segments(uint32_t tid, uint32_t rid,
                                                            double sampleRate) {
    if (!std::isfinite(sampleRate) || sampleRate < 8000.0 || sampleRate > 384000.0) {
        return false;
    }
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() == tid) { target = track; break; }
        }
    }
    return target && target->analyzeAudioNoteSegments(rid, sampleRate);
}

bool AuraUnifiedEngine::set_region_loop_count(uint32_t tid, uint32_t rid, uint32_t count) {
    if (count == 0 || count > 1024) return false;
    std::shared_ptr<Track> target;
    uint32_t before = 1;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() != tid) continue;
            const auto current = track->getRegionLoopCount(rid);
            if (!current) return false;
            target = track;
            before = *current;
            break;
        }
    }
    if (!target) return false;
    auto apply = [target, rid](uint32_t value) { target->setRegionLoopCount(rid, value); };
    m_undoManager.performAction(
        "Region Loop", [apply, before] { apply(before); }, [apply, count] { apply(count); });
    return true;
}

bool AuraUnifiedEngine::set_region_locked(uint32_t tid, uint32_t rid, bool locked) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) if (track->getId() == tid) return track->setRegionLocked(rid, locked);
    return false;
}
bool AuraUnifiedEngine::set_region_sync_group(uint32_t tid, uint32_t rid, uint32_t group) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) if (track->getId() == tid) return track->setRegionSyncGroup(rid, group);
    return false;
}
bool AuraUnifiedEngine::append_region_processing(uint32_t tid, uint32_t rid, uint32_t stepId, std::string_view operation, float parameter, bool enabled) {
    if (stepId == 0 || operation.empty() || operation.size() > 256 || !std::isfinite(parameter)) return false;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) if (track->getId() == tid)
        return track->appendProcessingStep(rid, {stepId, std::string(operation), parameter, enabled});
    return false;
}
bool AuraUnifiedEngine::clear_region_processing(uint32_t tid, uint32_t rid) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) if (track->getId() == tid) return track->clearProcessingHistory(rid);
    return false;
}

bool AuraUnifiedEngine::loadProject(std::string_view path) { 
    bool expectedHydration = false;
    if (!m_projectHydrationActive.compare_exchange_strong(
            expectedHydration, true, std::memory_order_acq_rel,
            std::memory_order_acquire)) {
        return false;
    }
    struct HydrationGuard {
        std::atomic<bool>& active;
        ~HydrationGuard() { active.store(false, std::memory_order_release); }
    } hydrationGuard{m_projectHydrationActive};

    const std::filesystem::path projectPath(path);
    const std::filesystem::path projectDir = projectPath.has_parent_path()
        ? projectPath.parent_path()
        : std::filesystem::current_path();
    auto state = ProjectSerializer::load(std::string(path)); 
    if (state.valid && std::isfinite(state.bpm) && state.bpm >= 20.0 && state.bpm <= 300.0) {
        const double projectSampleRate = static_cast<double>(state.sampleRate);
        const uint32_t configuredBlockSize = m_context ? m_context->blockSize : 0u;
        // A freshly created bridge may not have an audio device configured
        // yet. Project hydration still needs prepared track buffers so that
        // routing and sidechain links can be rebound deterministically.
        const uint32_t currentBlockSize = configuredBlockSize == 0
            ? 512u
            : configuredBlockSize;
        std::vector<uint32_t> trackIds;
        trackIds.reserve(state.tracks.size());
        for (const auto& ts : state.tracks) {
            if (ts.id == std::numeric_limits<uint32_t>::max() ||
                std::find(trackIds.begin(), trackIds.end(), ts.id) != trackIds.end()) return false;
            trackIds.push_back(ts.id);
        }
        std::unordered_set<uint32_t> serializedRegionIds;
        for (const auto& rs : state.regions) {
            if (rs.filePath.empty() || rs.sampleLength == 0 ||
                rs.samplePosition > std::numeric_limits<uint64_t>::max() - rs.sampleLength ||
                std::find(trackIds.begin(), trackIds.end(), rs.trackId) == trackIds.end()) return false;
            if (state.version >= 28 && (rs.id == 0 || !serializedRegionIds.insert(rs.id).second)) return false;
        }
        for (const auto& sidechain : state.sidechains) {
            if (sidechain.sourceId == sidechain.destinationId || sidechain.tapPoint > 2 ||
                std::find(trackIds.begin(), trackIds.end(), sidechain.sourceId) == trackIds.end() ||
                std::find(trackIds.begin(), trackIds.end(), sidechain.destinationId) == trackIds.end()) {
                return false;
            }
        }
        // Validate normal routing on a private graph before touching the
        // currently published project.  This keeps a malformed or cyclic
        // saved route from partially replacing the open session.
        Aura::Core::Engine::RoutingEngine replacementRouting;
        std::unordered_set<uint64_t> serializedRouteKeys;
        for (const auto& route : state.routes) {
            if (route.sourceId == route.destinationId ||
                route.sourceId >= Aura::Core::Engine::RoutingEngine::kMaxNodes ||
                route.destinationId >= Aura::Core::Engine::RoutingEngine::kMaxNodes ||
                !std::isfinite(route.gain) || route.gain <= 0.0f || route.gain > 2.0f ||
                std::find(trackIds.begin(), trackIds.end(), route.sourceId) == trackIds.end() ||
                std::find(trackIds.begin(), trackIds.end(), route.destinationId) == trackIds.end()) {
                return false;
            }
            const uint64_t routeKey = (static_cast<uint64_t>(route.sourceId) << 32) |
                                      static_cast<uint64_t>(route.destinationId);
            if (!serializedRouteKeys.insert(routeKey).second) return false;
            replacementRouting.addConnection(route.sourceId, route.destinationId, route.gain);
        }
        std::vector<Aura::Core::Engine::RoutingEngine::Connection> validatedRoutes;
        replacementRouting.copyConnections(validatedRoutes);
        if (validatedRoutes.size() != state.routes.size()) return false;
        set_playing(false);
        waitForAudioCallbacks();
        // Build and validate the replacement document before mutating the
        // currently published project.  Hydration can fail on a malformed
        // sidechain, plugin index, missing allocation, or unsupported audio
        // state; none of those failures may erase the user's open project.
        std::unique_lock<std::mutex> lock(m_tracksMutex);
        std::vector<std::shared_ptr<Track>> loadedTracks;
        loadedTracks.reserve(state.tracks.size());
        
        // 1. Hydrate Tracks
        for (const auto& ts : state.tracks) {
            const auto trackType = static_cast<Track::Type>(ts.type);
            std::shared_ptr<Track> t;
            if (trackType == Track::Type::Bus) {
                auto busTrack = std::make_shared<BusTrack>(
                    ts.id, ts.pluginName.empty() ? "Bus" : ts.pluginName, ts.id,
                    &m_sidechainManager);
                // Do not mutate the process/session BusSystem while the
                // replacement document is still provisional. If hydration
                // later fails, the old project must remain the only visible
                // routing state. Resolve the bus after the commit point.
                t = std::move(busTrack);
            } else {
                t = std::make_shared<Track>(ts.id,
                    ts.pluginName.empty() ? "Track" : ts.pluginName, trackType,
                    &m_sidechainManager);
            }
            // Plugin arrays are positional within a serialized track. Do not
            // silently truncate or shift state when an older/newer schema has
            // a different plugin count; that would restore one plugin's
            // preset onto another plugin after reload.
            if (ts.pluginData.size() % sizeof(uint32_t) != 0 ||
                ts.pluginStates.size() > ts.pluginData.size() / sizeof(uint32_t) ||
                ts.pluginBypass.size() > ts.pluginData.size() / sizeof(uint32_t) ||
                ts.sandboxedPluginStates.size() > ts.sandboxedPluginPaths.size()) {
                return false;
            }
            t->setVolume(ts.volume);
            t->setPan(ts.pan);
            t->setMuted(ts.muted);
            t->setSolo(ts.solo);
            t->setPhaseInverted(ts.phaseInvert);
            t->setRecordArmed(ts.recordArmed);
            t->setTrackDelaySamples(ts.trackDelaySamples);
            t->setSpatialPosition(ts.pan3d_x, ts.pan3d_y, ts.pan3d_z);
            if (!ts.volumeAutomation.empty()) t->setVolumeAutomation(ts.volumeAutomation);
            if (!ts.panAutomation.empty()) t->setPanAutomation(ts.panAutomation);
            if (!ts.trackDelayAutomation.empty()) t->setTrackDelayAutomation(ts.trackDelayAutomation);
            // pluginData stores plugin type IDs as little-endian u32 values.
            // 0xffffffff is the serialized marker for a sandbox plugin. Build
            // the processor chain in the same order in which it was saved;
            // restoring states before creating sandbox processors makes a
            // valid project fail when the first state targets that processor.
            size_t sandboxPathIndex = 0;
            for (size_t offset = 0; offset + sizeof(uint32_t) <= ts.pluginData.size();
                 offset += sizeof(uint32_t)) {
                const uint32_t pluginType = read_le_u32(ts.pluginData, offset);
                if (pluginType == 0xFFFFFFFFu) {
                    if (sandboxPathIndex >= ts.sandboxedPluginPaths.size()) return false;
                    // An unavailable external binary is represented as a
                    // quarantined/missing plugin and does not invalidate the
                    // rest of the project. The path is still consumed so the
                    // following serialized plugin keeps its position.
                    (void)t->addSandboxedPlugin(ts.sandboxedPluginPaths[sandboxPathIndex]);
                    ++sandboxPathIndex;
                } else if (!t->addPlugin(pluginType)) {
                    return false;
                }
            }
            // Older files may have sandbox paths without the marker in
            // pluginData. Preserve compatibility by appending those entries.
            while (sandboxPathIndex < ts.sandboxedPluginPaths.size()) {
                (void)t->addSandboxedPlugin(ts.sandboxedPluginPaths[sandboxPathIndex++]);
            }
            for (size_t stateIndex = 0; stateIndex < ts.pluginStates.size(); ++stateIndex) {
                if (stateIndex >= t->getPluginTypes().size()) break;
                // Missing sandbox binaries intentionally leave a hole in the
                // live chain; their state is retained in the serialized
                // document but cannot be applied until the plugin is relinked.
                (void)t->setPluginState(stateIndex, ts.pluginStates[stateIndex]);
            }
            for (size_t guiIndex = 0; guiIndex < ts.pluginGuiStates.size(); ++guiIndex) {
                if (guiIndex >= t->getPluginTypes().size()) break;
                (void)t->setPluginGuiState(guiIndex, ts.pluginGuiStates[guiIndex]);
            }
            for (size_t bypassIndex = 0; bypassIndex < ts.pluginBypass.size(); ++bypassIndex) {
                if (bypassIndex >= t->getPluginTypes().size()) break;
                (void)t->setPluginBypass(bypassIndex, ts.pluginBypass[bypassIndex] != 0);
            }
            loadedTracks.push_back(t);
        }
        
        // 2. Hydrate Regions
        uint32_t nextRegionId = 1;
        for (const auto& rs : state.regions) {
            for (auto& t : loadedTracks) {
                if (t->getId() == rs.trackId) {
                    const std::filesystem::path storedPath(rs.filePath);
                    const std::filesystem::path resolvedPath = storedPath.is_absolute()
                        ? storedPath
                        : projectDir / storedPath;
                    const std::string resolvedFilePath = resolvedPath.lexically_normal().string();
                    Region r;
                    if (state.version >= 28) {
                        r.id = rs.id;
                        if (r.id == std::numeric_limits<uint32_t>::max()) return false;
                        nextRegionId = std::max(nextRegionId, r.id + 1u);
                    } else {
                        r.id = nextRegionId++;
                    }
                    r.path = resolvedFilePath;
                    r.start = rs.samplePosition;
                    r.len = rs.sampleLength;
                    r.sourceOffset = rs.sourceOffset;
                    r.baseStart = rs.baseStart == 0 ? r.start : rs.baseStart;
                    r.baseSourceOffset = rs.baseSourceOffset;
                    r.baseLength = rs.baseLength == 0 ? r.len : rs.baseLength;
                    r.muted = rs.isMuted;
                    r.name = rs.name.empty() ? resolvedFilePath : rs.name;
                    r.clipGain = std::clamp(rs.clipGain, 0.0f, 2.0f);
                    r.fadeInSamples = std::min(rs.fadeInSamples, r.len);
                    r.fadeOutSamples = std::min(rs.fadeOutSamples, r.len);
                    r.reverse = rs.reverse;
                    r.warpRatio = rs.warpRatio;
                    r.pitchSemitones = rs.pitchSemitones;
                    r.loopCount = rs.loopCount;
                    r.locked = rs.locked;
                    r.syncGroup = rs.syncGroup;
                    r.processingHistory = rs.processingHistory;
                    r.audioNoteSegments = rs.audioNoteSegments;
                    r.rangeEdits.clear();
                    r.rangeEdits.reserve(std::min<size_t>(rs.rangeEdits.size(), 4096));
                    for (const auto& edit : rs.rangeEdits) {
                        // Reject malformed persisted edits before they reach
                        // the real-time renderer.  This mirrors serializer
                        // validation and protects hand-edited project files.
                        if (edit.start >= edit.end || (r.len > 0 && edit.end > r.len) ||
                            !std::isfinite(edit.gain) || edit.gain < 0.0f || edit.gain > 16.0f ||
                            edit.fadeIn > edit.end - edit.start || edit.fadeOut > edit.end - edit.start) {
                            continue;
                        }
                        if (r.rangeEdits.size() >= 4096) break;
                        r.rangeEdits.push_back({edit.start, edit.end, edit.gain, edit.fadeIn, edit.fadeOut});
                    }
                    // Hydrate audio during project load so an opened project
                    // is immediately playable. Missing files remain visible
                    // as unloaded regions and can be repaired via Relink.
                    std::error_code fileError;
                    if (std::filesystem::is_regular_file(resolvedPath, fileError) && !fileError) {
                        auto audio = m_audioDecoderManager.importFile(resolvedFilePath);
                        if (audio && audio->getNumSamples() > 0 && audio->getNumChannels() > 0) {
                            r.audio = std::move(audio);
                            if (r.sourceOffset >= r.audio->getNumSamples()) continue;
                            r.len = std::min(r.len, r.audio->getNumSamples() - r.sourceOffset);
                        }
                    }
                    // The persisted region length may have been larger than
                    // the decoded media (for example after an external file
                    // was replaced).  Revalidate range edits against the
                    // final playable length so the renderer never evaluates
                    // stale offsets beyond the audio buffer.
                    if (!r.rangeEdits.empty()) {
                        const auto playableLength = r.len;
                        r.rangeEdits.erase(
                            std::remove_if(r.rangeEdits.begin(), r.rangeEdits.end(),
                                [playableLength](const Region::RangeEdit& edit) {
                                    return edit.start >= edit.end || edit.end > playableLength ||
                                           edit.fadeIn > edit.end - edit.start ||
                                           edit.fadeOut > edit.end - edit.start;
                                }),
                            r.rangeEdits.end());
                    }
                    t->addRegion(r);
                    break;
                }
            }
        }
        // Preflight sidechain bindings while the old project is still the
        // published document. A malformed plugin index or cyclic topology
        // must not fail after the new tracks have been swapped into the live
        // engine.
        for (const auto& sidechain : state.sidechains) {
            const auto source = std::find_if(loadedTracks.begin(), loadedTracks.end(),
                [&sidechain](const auto& track) { return track->getId() == sidechain.sourceId; });
            const auto destination = std::find_if(loadedTracks.begin(), loadedTracks.end(),
                [&sidechain](const auto& track) { return track->getId() == sidechain.destinationId; });
            if (source == loadedTracks.end() || destination == loadedTracks.end() ||
                sidechain.pluginIndex >= (*destination)->getPluginTypes().size() ||
                sidechain.tapPoint > 2) return false;
        }
        for (const auto& candidate : state.sidechains) {
            std::vector<uint32_t> pending{candidate.destinationId};
            std::unordered_set<uint32_t> visited;
            while (!pending.empty()) {
                const uint32_t node = pending.back();
                pending.pop_back();
                if (node == candidate.sourceId) return false;
                if (!visited.insert(node).second) continue;
                for (const auto& route : state.sidechains) {
                    if (route.sourceId == node) pending.push_back(route.destinationId);
                }
            }
        }

        // Prepare the detached replacement tracks before the commit point.
        // This catches invalid plugin/buffer state while the old project is
        // still published; a later prepare failure must not leave the engine
        // with an empty or half-hydrated graph.
        for (const auto& track : loadedTracks) {
            if (!track) return false;
            track->prepareToPlay(projectSampleRate, currentBlockSize);
            if (!track->canProcess(currentBlockSize)) return false;
        }
        
        uint32_t nextTrackId = 1;
        for (const auto& track : loadedTracks) {
            // Track ID 0 is valid for the bootstrap/template track. Only the
            // uint32 max sentinel is reserved; rejecting zero here made a
            // native project pass serialization but fail hydration on reload.
            if (!track || track->getId() == std::numeric_limits<uint32_t>::max()) return false;
            nextTrackId = std::max(nextTrackId, track->getId() + 1u);
        }
        // Commit point: all validation and hydration above completed while
        // the old project remained published.  Only now replace the graph,
        // reset project-scoped managers, and publish fresh ID allocators.
        auto& sidechainManager = m_sidechainManager;
        for (const auto& track : m_tracks) {
            sidechainManager.removeLinksForTrack(track->getId());
        }
        m_commandQueue.clear();
        m_pdcManager.resetForProject();
        sidechainManager.resetForProject();
        m_routingEngine.resetForProject();
        m_controlRoom.resetForProject();
        m_sidechainRoutes.clear();
        m_regionIdCounter.store(nextRegionId, std::memory_order_release);
        m_trackIdCounter.store(nextTrackId, std::memory_order_release);
        m_tracks = std::move(loadedTracks);
        for (const auto& route : state.routes) {
            m_routingEngine.addConnection(route.sourceId, route.destinationId, route.gain);
        }
        for (const auto& track : m_tracks) {
            if (auto* busTrack = dynamic_cast<BusTrack*>(track.get())) {
                busTrack->resolveBus(m_busSystem.registerBus(busTrack->busId()));
            }
        }
        lock.unlock();
        m_tempo.store((float)state.bpm);
        m_rootNote.store(state.rootNote, std::memory_order_relaxed);
        m_scaleType.store(state.scaleType, std::memory_order_relaxed);
        // Restore timeline markers as part of the document, not as transient UI state.
        m_arrangement.clearMarkers();
        for (const auto& marker : state.markers) {
            m_arrangement.upsertMarker({marker.sample, marker.name, marker.color});
        }
        std::vector<Composition::ArrangerPart> arrangerParts;
        arrangerParts.reserve(state.arrangerParts.size());
        for (const auto& part : state.arrangerParts)
            arrangerParts.push_back({part.start, part.length, part.name, part.repeats});
        m_arrangement.setArrangerParts(std::move(arrangerParts));
        m_tempoMap.clear(projectSampleRate, state.bpm);
        for (const auto& event : state.tempoEvents)
            m_tempoMap.addTempo(event.sample, event.bpm, projectSampleRate, event.ramp != 0);
        // A project carries its own timeline rate. Re-prepare every track and
        // DSP component so opening a 48/96 kHz project does not leave the
        // engine running at the previous device rate.
        prepareToPlay(projectSampleRate, currentBlockSize);
        for (const auto& sidechain : state.sidechains) {
            if (!set_sidechain_link(sidechain.sourceId, sidechain.destinationId,
                                    sidechain.pluginIndex, sidechain.tapPoint, true)) {
                return false;
            }
        }
        // A loaded project starts a fresh editing history. Keeping commands
        // from the previous document would let Undo mutate the wrong graph.
        m_undoManager.clear();
        return true;
    }
    return false;
}

bool AuraUnifiedEngine::saveProject(std::string_view path) { 
    ProjectSerializer::ProjectState state;
    state.bpm = m_tempo.load(); 
    state.rootNote = m_rootNote.load(std::memory_order_relaxed);
    state.scaleType = m_scaleType.load(std::memory_order_relaxed);
    state.sampleRate = static_cast<uint32_t>(m_sampleRate.load());
    
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for(auto& t : m_tracks) {
        ProjectSerializer::TrackState ts;
        ts.id = t->getId(); 
        ts.type = static_cast<uint32_t>(t->getType());
        ts.volume = t->getVolume(); 
        ts.pan = t->getPan();
        ts.muted = t->isMuted();
        ts.solo = t->isSolo();
        ts.phaseInvert = t->isPhaseInverted();
        ts.recordArmed = t->isRecordArmed();
        ts.trackDelaySamples = t->getTrackDelaySamples();
        ts.pan3d_x = t->getSpatialX();
        ts.pan3d_y = t->getSpatialY();
        ts.pan3d_z = t->getSpatialZ();
        // Drain callback-side write automation before serializing so a save
        // cannot silently omit the last recorded control-surface blocks.
        t->flushRecordedAutomation();
        ts.volumeAutomation = t->getVolumeAutomationPoints();
        ts.panAutomation = t->getPanAutomationPoints();
        ts.trackDelayAutomation = t->getTrackDelayAutomationPoints();
        ts.pluginName = t->getName();
        for (uint32_t pluginType : t->getPluginTypes()) {
            append_le_u32(ts.pluginData, pluginType);
        }
        ts.pluginStates = t->getPluginStates();
        ts.pluginGuiStates = t->getPluginGuiStates();
        ts.pluginBypass.reserve(t->getPluginTypes().size());
        for (uint32_t pluginIndex = 0; pluginIndex < t->getPluginTypes().size(); ++pluginIndex) {
            ts.pluginBypass.push_back(t->getPluginBypass(pluginIndex) ? 1 : 0);
        }
        ts.sandboxedPluginPaths = t->getSandboxedPluginPaths();
        ts.sandboxedPluginStates = t->getSandboxedPluginStates();
        state.tracks.push_back(ts);
        for (const auto& region : t->getRegions()) {
            ProjectSerializer::RegionState rs;
            rs.id = region.id;
            rs.trackId = t->getId();
            rs.samplePosition = region.start;
            rs.sampleLength = region.len;
            rs.sourceOffset = region.sourceOffset;
            rs.baseStart = region.baseStart;
            rs.baseSourceOffset = region.baseSourceOffset;
            rs.baseLength = region.baseLength;
            rs.isMuted = region.muted;
            rs.filePath = region.path;
            rs.name = region.name;
            rs.clipGain = region.clipGain;
            rs.fadeInSamples = region.fadeInSamples;
            rs.fadeOutSamples = region.fadeOutSamples;
            rs.reverse = region.reverse;
            rs.warpRatio = region.warpRatio;
            rs.pitchSemitones = region.pitchSemitones;
            rs.loopCount = region.loopCount;
            rs.locked = region.locked;
            rs.syncGroup = region.syncGroup;
            rs.processingHistory = region.processingHistory;
            rs.audioNoteSegments = region.audioNoteSegments;
            rs.rangeEdits.clear();
            for (const auto& edit : region.rangeEdits) {
                rs.rangeEdits.push_back({edit.start, edit.end, edit.gain, edit.fadeIn, edit.fadeOut});
            }
            state.regions.push_back(rs);
        }
    }
    for (const auto& sidechain : m_sidechainRoutes) {
        ProjectSerializer::SidechainState serialized;
        serialized.sourceId = sidechain.sourceId;
        serialized.destinationId = sidechain.destinationId;
        serialized.pluginIndex = sidechain.pluginIndex;
        // Route identity is persisted independently from native buffer
        // pointers; those are rebound after the project is loaded.
        serialized.tapPoint = sidechain.tapPoint;
        state.sidechains.push_back(serialized);
    }
    for (const auto& marker : m_arrangement.markers()) {
        ProjectSerializer::MarkerState serialized;
        serialized.sample = marker.sample;
        serialized.name = marker.name;
        serialized.color = marker.color;
        state.markers.push_back(std::move(serialized));
    }
    for (const auto& part : m_arrangement.arrangerParts()) {
        ProjectSerializer::ArrangerPartState serialized;
        serialized.start = part.start; serialized.length = part.length;
        serialized.name = part.name; serialized.repeats = part.repeats;
        state.arrangerParts.push_back(std::move(serialized));
    }
    for (const auto& event : m_tempoMap.getEvents()) {
        ProjectSerializer::TempoEventState serialized;
        serialized.sample = event.samplePos; serialized.bpm = event.bpm; serialized.ramp = event.ramp ? 1 : 0;
        state.tempoEvents.push_back(serialized);
    }
    std::vector<Aura::Core::Engine::RoutingEngine::Connection> routeConnections;
    m_routingEngine.copyConnections(routeConnections);
    state.routes.reserve(routeConnections.size());
    for (const auto& connection : routeConnections) {
        ProjectSerializer::RouteState serialized;
        serialized.sourceId = connection.sourceId;
        serialized.destinationId = connection.destId;
        serialized.gain = connection.gain;
        state.routes.push_back(serialized);
    }
    
    // Note: SBF-v6 enables CRC32 and Endian-Safety automatically in saveAtomic
    return ProjectSerializer::saveAtomic(std::string(path), state);
}


rust::Vec<uint8_t> AuraUnifiedEngine::get_video_frame() {
    rust::Vec<uint8_t> frame;
#if defined(__APPLE__)
    std::vector<uint8_t> rgba;
    uint32_t width = 0, height = 0;
    if (::Aura::Core::Engine::VideoBridge::copyCurrentFrame(rgba, width, height) &&
        width > 0 && height > 0 && rgba.size() == static_cast<size_t>(width) * height * 4u) {
        // Packet: little-endian width, height, followed by packed RGBA8.
        frame.reserve(8u + rgba.size());
        for (uint32_t shift = 0; shift < 32; shift += 8) frame.push_back(static_cast<uint8_t>((width >> shift) & 0xffu));
        for (uint32_t shift = 0; shift < 32; shift += 8) frame.push_back(static_cast<uint8_t>((height >> shift) & 0xffu));
        for (uint8_t value : rgba) frame.push_back(value);
        return frame;
    }
#endif
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    if (!std::isfinite(sampleRate) || sampleRate <= 0.0) return frame;
#if !defined(__APPLE__)
    ::Aura::Core::Scheduler::VideoFrame source;
    const double seconds = static_cast<double>(m_playhead.load(std::memory_order_relaxed)) / sampleRate;
    if (!::Aura::Core::Scheduler::VideoEngine::getInstance().copyFrameAt(seconds, source) ||
        source.width <= 0 || source.height <= 0 || source.rgbData.empty()) return frame;
    const size_t pixelCount = static_cast<size_t>(source.width) * static_cast<size_t>(source.height);
    if (source.rgbData.size() != pixelCount * 3u || pixelCount > (16u * 1024u * 1024u)) return frame;
    frame.reserve(8u + pixelCount * 4u);
    for (uint32_t shift = 0; shift < 32; shift += 8) frame.push_back(static_cast<uint8_t>((static_cast<uint32_t>(source.width) >> shift) & 0xffu));
    for (uint32_t shift = 0; shift < 32; shift += 8) frame.push_back(static_cast<uint8_t>((static_cast<uint32_t>(source.height) >> shift) & 0xffu));
    for (size_t i = 0; i < pixelCount; ++i) {
        frame.push_back(source.rgbData[i * 3u]);
        frame.push_back(source.rgbData[i * 3u + 1u]);
        frame.push_back(source.rgbData[i * 3u + 2u]);
        frame.push_back(255u);
    }
    return frame;
#else
    return frame;
#endif
}
