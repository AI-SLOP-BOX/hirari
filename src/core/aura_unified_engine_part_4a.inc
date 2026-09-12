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
