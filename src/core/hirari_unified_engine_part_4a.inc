bool HirariUnifiedEngine::move_region(uint32_t tid, uint32_t rid, double startBeat) {
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
    const uint64_t sourceTrim = moved.sourceOffset >= moved.baseSourceOffset
        ? moved.sourceOffset - moved.baseSourceOffset : 0;
    const long double timelineTrim = std::ceil(
        static_cast<long double>(sourceTrim) / moved.sourceFramesPerTimelineFrame());
    if (!std::isfinite(static_cast<double>(timelineTrim)) || timelineTrim < 0.0L ||
        timelineTrim > static_cast<long double>(moved.start)) return false;
    moved.baseStart = moved.start - static_cast<uint64_t>(timelineTrim);
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

bool HirariUnifiedEngine::move_region_sync_group(uint32_t tid, uint32_t rid, double startSample) {
    if (tid == 0 || rid == 0 || !std::isfinite(startSample) || startSample < 0.0 ||
        startSample >= static_cast<double>(UINT64_MAX)) return false;

    struct RegionState {
        std::shared_ptr<Track> track;
        Region region;
    };
    std::vector<RegionState> before;
    std::vector<RegionState> after;
    std::vector<uint64_t> regionStarts;
    std::vector<uint64_t> baseStarts;
    std::vector<uint64_t> movedStarts;
    std::vector<uint64_t> movedBaseStarts;
    uint32_t group = 0;
    uint64_t anchorStart = 0;
    const uint64_t newAnchorStart = static_cast<uint64_t>(startSample);
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        std::shared_ptr<Track> anchorTrack;
        for (const auto& track : m_tracks) {
            if (!track || track->getId() != tid) continue;
            anchorTrack = track;
            for (const auto& region : track->getRegions()) {
                if (region.id == rid) {
                    group = region.syncGroup;
                    anchorStart = region.start;
                    if (region.locked) return false;
                    break;
                }
            }
            break;
        }
        if (!anchorTrack || group == 0) return false;

        for (const auto& track : m_tracks) {
            if (!track) continue;
            for (const auto& region : track->getRegions()) {
                if (region.syncGroup != group) continue;
                if (region.locked) return false;
                before.push_back({track, region});
                after.push_back({track, region});
                regionStarts.push_back(region.start);
                baseStarts.push_back(region.baseStart);
            }
        }
    }
    if (before.size() < 2 || before.size() != after.size()) return false;
    movedStarts.resize(regionStarts.size());
    movedBaseStarts.resize(baseStarts.size());
    if (!hirari_region_sync_move_positions(
            regionStarts.data(), baseStarts.data(), regionStarts.size(),
            anchorStart, newAnchorStart, movedStarts.data(), movedBaseStarts.data())) return false;
    for (size_t index = 0; index < after.size(); ++index) {
        after[index].region.start = movedStarts[index];
        after[index].region.baseStart = movedBaseStarts[index];
    }
    const auto apply = [this](const std::vector<RegionState>& states) {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& state : states) {
            state.track->removeRegion(state.region.id);
            state.track->addRegion(state.region);
            std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
            m_peakCache.erase(state.region.id);
        }
    };
    m_undoManager.performAction(
        "Move Region Sync Group",
        [apply, before] { apply(before); },
        [apply, after] { apply(after); });
    return true;
}

bool HirariUnifiedEngine::remove_region(uint32_t tid, uint32_t rid) {
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

bool HirariUnifiedEngine::set_region_gain(uint32_t tid, uint32_t rid, float gain) {
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

bool HirariUnifiedEngine::set_region_muted(uint32_t tid, uint32_t rid, bool muted) {
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

bool HirariUnifiedEngine::set_region_comp_ranges(
    uint32_t tid,
    uint32_t rid,
    rust::Slice<const uint64_t> packedRanges,
    bool managed) {
    if (packedRanges.size() % 4 != 0 || packedRanges.size() / 4 > 4096) return false;
    std::vector<Region::CompRange> ranges;
    ranges.reserve(packedRanges.size() / 4);
    for (size_t index = 0; index < packedRanges.size(); index += 4) {
        ranges.push_back({packedRanges[index], packedRanges[index + 1],
                          packedRanges[index + 2], packedRanges[index + 3]});
    }
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() == tid) {
                target = track;
                break;
            }
        }
    }
    return target && target->setRegionCompRanges(rid, ranges, managed);
}

bool HirariUnifiedEngine::set_region_fades(uint32_t tid, uint32_t rid, float fadeIn, float fadeOut) {
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

bool HirariUnifiedEngine::set_region_range_edit(uint32_t tid, uint32_t rid, uint64_t start,
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

bool HirariUnifiedEngine::clear_region_range_edit(uint32_t tid, uint32_t rid, uint64_t start,
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

bool HirariUnifiedEngine::clear_region_range_edits(uint32_t tid, uint32_t rid) {
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

bool HirariUnifiedEngine::set_region_reverse(uint32_t tid, uint32_t rid, bool reverse) {
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

bool HirariUnifiedEngine::set_region_trim(uint32_t tid, uint32_t rid, float startNorm, float endNorm) {
    if (!std::isfinite(startNorm) || !std::isfinite(endNorm)) return false;
    const float boundedStart = std::clamp(startNorm, 0.0f, 1.0f);
    const float boundedEnd = std::clamp(endNorm, 0.0f, 1.0f);
    if (boundedEnd <= boundedStart) return false;
    std::shared_ptr<Track> target;
    std::vector<::Hirari::Core::Engine::Region::WarpMarker> beforeMarkers;
    float beforeStart = 0.0f;
    float beforeEnd = 1.0f;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() != tid) continue;
            for (const auto& region : track->getRegions()) {
                if (region.id == rid && region.baseLength > 0) {
                    target = track;
                    beforeMarkers = region.warpMarkers;
                    const uint64_t sourceLength = region.sourceLength > 0
                        ? region.sourceLength : region.len;
                    beforeStart = static_cast<float>(region.sourceOffset - region.baseSourceOffset) /
                                  static_cast<float>(region.baseLength);
                    beforeEnd = static_cast<float>(region.sourceOffset - region.baseSourceOffset + sourceLength) /
                                static_cast<float>(region.baseLength);
                    break;
                }
            }
            break;
        }
    }
    if (!target) return false;
    auto apply = [target, rid](float start, float end, const auto& markers) {
        if (target->setRegionTrim(rid, start, end)) {
            (void)target->setRegionWarpMarkers(rid, markers);
        }
    };
    m_undoManager.performAction(
        "Trim Region", [apply, beforeStart, beforeEnd, beforeMarkers] {
            apply(beforeStart, beforeEnd, beforeMarkers);
        },
        [apply, boundedStart, boundedEnd] {
            apply(boundedStart, boundedEnd,
                  std::vector<::Hirari::Core::Engine::Region::WarpMarker>{});
        });
    return true;
}

bool HirariUnifiedEngine::set_region_warp_ratio(uint32_t tid, uint32_t rid, double ratio) {
    if (!std::isfinite(ratio) || ratio < 0.5 || ratio > 2.0) return false;
    std::shared_ptr<Track> target;
    std::vector<::Hirari::Core::Engine::Region::WarpMarker> beforeMarkers;
    double before = 1.0;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() != tid) continue;
            const auto current = track->getRegionWarpRatio(rid);
            if (!current) return false;
            target = track;
            before = *current;
            beforeMarkers = track->getRegionWarpMarkers(rid);
            break;
        }
    }
    if (!target) return false;
    auto apply = [target, rid](double value, const auto& markers) {
        if (target->setRegionWarpRatio(rid, value)) {
            (void)target->setRegionWarpMarkers(rid, markers);
        }
    };
    m_undoManager.performAction(
        "Flex Warp Ratio", [apply, before, beforeMarkers] { apply(before, beforeMarkers); },
        [apply, ratio] {
            apply(ratio, std::vector<::Hirari::Core::Engine::Region::WarpMarker>{});
        });
    return true;
}

bool HirariUnifiedEngine::set_region_pitch_preserve_warp(uint32_t tid, uint32_t rid, bool enabled) {
    std::shared_ptr<Track> target;
    bool before = false;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() != tid) continue;
            const auto current = track->getRegionPitchPreserveWarp(rid);
            if (!current) return false;
            target = track;
            before = *current;
            break;
        }
    }
    if (!target) return false;
    auto apply = [target, rid](bool value) { target->setRegionPitchPreserveWarp(rid, value); };
    m_undoManager.performAction(
        "Preserve Warp Pitch", [apply, before] { apply(before); }, [apply, enabled] { apply(enabled); });
    return true;
}

bool HirariUnifiedEngine::set_region_pitch_semitones(uint32_t tid, uint32_t rid, float semitones) {
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

bool HirariUnifiedEngine::set_region_audio_note_segment(
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
    std::vector<::hirari::editing::AudioNoteSegment> before;
    if (!target->copyAudioNoteSegments(rid, before)) return false;
    ::hirari::editing::AudioNoteSegment segment;
    segment.startSeconds = startSeconds;
    segment.endSeconds = endSeconds;
    const auto existing = std::find_if(before.begin(), before.end(),
        [startSeconds, endSeconds](const auto& current) {
            return std::abs(current.startSeconds - startSeconds) < 1e-9 &&
                   std::abs(current.endSeconds - endSeconds) < 1e-9;
        });
    if (existing != before.end()) segment = *existing;
    segment.startSeconds = startSeconds;
    segment.endSeconds = endSeconds;
    segment.setPitchOffset(pitchOffsetCents);
    segment.setFormantOffset(formantOffsetCents);
    if (!target->upsertAudioNoteSegment(rid, std::move(segment))) return false;
    std::vector<::hirari::editing::AudioNoteSegment> after;
    if (!target->copyAudioNoteSegments(rid, after)) return false;
    const auto apply = [target, rid](const auto& segments) {
        (void)target->setAudioNoteSegments(rid, segments);
    };
    const auto actionName = "Edit Audio Pitch Segments " +
        std::to_string(tid) + ":" + std::to_string(rid);
    m_undoManager.recordAppliedAction(
        actionName, [apply, before] { apply(before); },
        [apply, after] { apply(after); }, true);
    return true;
}

bool HirariUnifiedEngine::set_region_audio_note_anchor(
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
    ::hirari::editing::AudioNoteAnchor anchor;
    anchor.positionSeconds = positionSeconds;
    anchor.pitchCents = pitchCents;
    anchor.formantCents = formantCents;
    std::vector<::hirari::editing::AudioNoteSegment> before;
    if (!target->copyAudioNoteSegments(rid, before) ||
        !target->upsertAudioNoteAnchor(rid, segmentStartSeconds, anchor)) return false;
    std::vector<::hirari::editing::AudioNoteSegment> after;
    if (!target->copyAudioNoteSegments(rid, after)) return false;
    const auto apply = [target, rid](const auto& segments) {
        (void)target->setAudioNoteSegments(rid, segments);
    };
    const auto actionName = "Edit Audio Pitch Curve " +
        std::to_string(tid) + ":" + std::to_string(rid);
    m_undoManager.recordAppliedAction(
        actionName, [apply, before] { apply(before); },
        [apply, after] { apply(after); }, true);
    return true;
}

bool HirariUnifiedEngine::set_region_audio_note_formant_anchor(
    uint32_t tid, uint32_t rid, double segmentStartSeconds,
    double positionSeconds, double formantCents) {
    if (!std::isfinite(segmentStartSeconds) || segmentStartSeconds < 0.0 ||
        !std::isfinite(positionSeconds) || positionSeconds < 0.0 ||
        !std::isfinite(formantCents) || std::abs(formantCents) > 2400.0) return false;
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() == tid) { target = track; break; }
        }
    }
    if (!target) return false;
    std::vector<::hirari::editing::AudioNoteSegment> before;
    if (!target->copyAudioNoteSegments(rid, before)) return false;
    const auto segment = std::find_if(before.begin(), before.end(), [&](const auto& item) {
        return std::abs(item.startSeconds - segmentStartSeconds) < 1e-9;
    });
    if (segment == before.end() || positionSeconds < segment->startSeconds ||
        positionSeconds > segment->endSeconds) return false;
    ::hirari::editing::AudioNoteAnchor anchor{};
    if (!hirari_audio_note_curve_component_anchor(
            segment->anchors.data(), segment->anchors.size(), segment->startSeconds,
            segment->endSeconds, positionSeconds, formantCents, 1,
            segment->pitchOffsetCents, segment->formantOffsetCents, &anchor)) return false;
    if (!target->upsertAudioNoteAnchor(rid, segmentStartSeconds, anchor)) return false;
    std::vector<::hirari::editing::AudioNoteSegment> after;
    if (!target->copyAudioNoteSegments(rid, after)) return false;
    const auto apply = [target, rid](const auto& segments) {
        (void)target->setAudioNoteSegments(rid, segments);
    };
    const auto actionName = "Edit Audio Formant Curve " +
        std::to_string(tid) + ":" + std::to_string(rid);
    m_undoManager.recordAppliedAction(
        actionName, [apply, before] { apply(before); },
        [apply, after] { apply(after); }, true);
    return true;
}

bool HirariUnifiedEngine::set_region_audio_note_pitch_anchor(
    uint32_t tid, uint32_t rid, double segmentStartSeconds,
    double positionSeconds, double pitchCents) {
    if (!std::isfinite(segmentStartSeconds) || segmentStartSeconds < 0.0 ||
        !std::isfinite(positionSeconds) || positionSeconds < 0.0 ||
        !std::isfinite(pitchCents) || std::abs(pitchCents) > 4800.0) return false;
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() == tid) { target = track; break; }
        }
    }
    if (!target) return false;
    std::vector<::hirari::editing::AudioNoteSegment> before;
    if (!target->copyAudioNoteSegments(rid, before)) return false;
    const auto segment = std::find_if(before.begin(), before.end(), [&](const auto& item) {
        return std::abs(item.startSeconds - segmentStartSeconds) < 1e-9;
    });
    if (segment == before.end() || positionSeconds < segment->startSeconds ||
        positionSeconds > segment->endSeconds) return false;
    ::hirari::editing::AudioNoteAnchor anchor{};
    if (!hirari_audio_note_curve_component_anchor(
            segment->anchors.data(), segment->anchors.size(), segment->startSeconds,
            segment->endSeconds, positionSeconds, pitchCents, 0,
            segment->pitchOffsetCents, segment->formantOffsetCents, &anchor)) return false;
    if (!target->upsertAudioNoteAnchor(rid, segmentStartSeconds, anchor)) return false;
    std::vector<::hirari::editing::AudioNoteSegment> after;
    if (!target->copyAudioNoteSegments(rid, after)) return false;
    const auto apply = [target, rid](const auto& segments) {
        (void)target->setAudioNoteSegments(rid, segments);
    };
    const auto actionName = "Edit Audio Pitch Curve " +
        std::to_string(tid) + ":" + std::to_string(rid);
    m_undoManager.recordAppliedAction(
        actionName, [apply, before] { apply(before); },
        [apply, after] { apply(after); }, true);
    return true;
}

bool HirariUnifiedEngine::move_region_audio_note_anchor(
    uint32_t tid, uint32_t rid, double segmentStartSeconds,
    double oldPositionSeconds, double newPositionSeconds,
    double valueCents, bool editFormant) {
    const double maxCents = editFormant ? 2400.0 : 4800.0;
    if (!std::isfinite(segmentStartSeconds) || segmentStartSeconds < 0.0 ||
        !std::isfinite(oldPositionSeconds) || oldPositionSeconds < 0.0 ||
        !std::isfinite(newPositionSeconds) || newPositionSeconds < 0.0 ||
        !std::isfinite(valueCents) || std::abs(valueCents) > maxCents) return false;
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() == tid) { target = track; break; }
        }
    }
    if (!target) return false;
    std::vector<::hirari::editing::AudioNoteSegment> before;
    if (!target->copyAudioNoteSegments(rid, before)) return false;
    auto segment = std::find_if(before.begin(), before.end(), [&](const auto& item) {
        return std::abs(item.startSeconds - segmentStartSeconds) < 1e-9;
    });
    if (segment == before.end() || newPositionSeconds < segment->startSeconds ||
        newPositionSeconds > segment->endSeconds) return false;
    auto movedAnchors = segment->anchors;
    const size_t movedCount = hirari_audio_note_curve_move_anchor(
        movedAnchors.data(), movedAnchors.size(), segment->startSeconds, segment->endSeconds,
        oldPositionSeconds, newPositionSeconds, valueCents, editFormant ? 1 : 0,
        segment->pitchOffsetCents, segment->formantOffsetCents);
    if (movedCount > movedAnchors.size()) return false;
    movedAnchors.resize(movedCount);
    segment->anchors = std::move(movedAnchors);
    segment->rebuildPitchRatioIntegral();
    if (!target->upsertAudioNoteSegment(rid, *segment)) return false;
    std::vector<::hirari::editing::AudioNoteSegment> after;
    if (!target->copyAudioNoteSegments(rid, after)) return false;
    const auto apply = [target, rid](const auto& segments) {
        (void)target->setAudioNoteSegments(rid, segments);
    };
    const auto actionName = std::string(editFormant ? "Move Audio Formant Anchor " :
                                                    "Move Audio Pitch Anchor ") +
        std::to_string(tid) + ":" + std::to_string(rid);
    m_undoManager.recordAppliedAction(
        actionName, [apply, before] { apply(before); },
        [apply, after] { apply(after); }, true);
    return true;
}

bool HirariUnifiedEngine::clear_region_audio_note_segments(uint32_t tid, uint32_t rid) {
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() == tid) { target = track; break; }
        }
    }
    if (!target) return false;
    std::vector<::hirari::editing::AudioNoteSegment> before;
    if (!target->copyAudioNoteSegments(rid, before) ||
        !target->clearAudioNoteSegments(rid)) return false;
    const auto apply = [target, rid](const auto& segments) {
        (void)target->setAudioNoteSegments(rid, segments);
    };
    m_undoManager.recordAppliedAction(
        "Clear Audio Pitch Segments", [apply, before] { apply(before); },
        [apply] { (void)apply(std::vector<::hirari::editing::AudioNoteSegment>{}); });
    return true;
}

bool HirariUnifiedEngine::warp_region_audio_note_segment(
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
    if (!target) return false;
    std::vector<::hirari::editing::AudioNoteSegment> before;
    if (!target->copyAudioNoteSegments(rid, before) ||
        !target->warpAudioNoteSegment(rid, segmentStartSeconds,
                                      newStartSeconds, newEndSeconds)) return false;
    std::vector<::hirari::editing::AudioNoteSegment> after;
    if (!target->copyAudioNoteSegments(rid, after)) return false;
    const auto apply = [target, rid](const auto& segments) {
        (void)target->setAudioNoteSegments(rid, segments);
    };
    m_undoManager.recordAppliedAction(
        "Warp Audio Pitch Segment", [apply, before] { apply(before); },
        [apply, after] { apply(after); });
    return true;
}

bool HirariUnifiedEngine::remove_region_audio_note_segment(uint32_t tid, uint32_t rid,
                                                          double segmentStartSeconds) {
    if (!std::isfinite(segmentStartSeconds) || segmentStartSeconds < 0.0) return false;
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks)
            if (track->getId() == tid) { target = track; break; }
    }
    if (!target) return false;
    std::vector<::hirari::editing::AudioNoteSegment> before;
    if (!target->copyAudioNoteSegments(rid, before) ||
        !target->removeAudioNoteSegment(rid, segmentStartSeconds)) return false;
    std::vector<::hirari::editing::AudioNoteSegment> after;
    if (!target->copyAudioNoteSegments(rid, after)) return false;
    const auto apply = [target, rid](const auto& segments) {
        (void)target->setAudioNoteSegments(rid, segments);
    };
    m_undoManager.recordAppliedAction(
        "Remove Audio Pitch Segment", [apply, before] { apply(before); },
        [apply, after] { apply(after); });
    return true;
}

bool HirariUnifiedEngine::analyze_region_audio_note_segments(uint32_t tid, uint32_t rid,
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
    if (!target) return false;
    if (!target->beginAudioPitchAnalysis()) return false;
    std::vector<::hirari::editing::AudioNoteSegment> before;
    if (!target->copyAudioNoteSegments(rid, before)) {
        target->finishAudioPitchAnalysis();
        return false;
    }
    const bool analyzed = target->analyzeAudioNoteSegments(rid, sampleRate);
    std::vector<::hirari::editing::AudioNoteSegment> after;
    const bool copied = analyzed && target->copyAudioNoteSegments(rid, after);
    target->finishAudioPitchAnalysis();
    if (!copied) return false;
    const auto apply = [target, rid](const auto& segments) {
        (void)target->setAudioNoteSegments(rid, segments);
    };
    m_undoManager.recordAppliedAction(
        "Analyze Audio Pitch", [apply, before] { apply(before); },
        [apply, after] { apply(after); });
    return true;
}

bool HirariUnifiedEngine::start_region_audio_note_analysis(
    uint32_t tid, uint32_t rid, double sampleRate) {
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
    if (!target || !target->beginAudioPitchAnalysis()) return false;
    auto job = std::make_shared<AudioPitchAnalysisJob>();
    job->target = target;
    if (!target->copyAudioNoteSegments(rid, job->before)) {
        target->finishAudioPitchAnalysis();
        return false;
    }
    const uint64_t key = (static_cast<uint64_t>(tid) << 32) | rid;
    {
        std::lock_guard<std::mutex> lock(m_audioPitchAnalysisMutex);
        size_t activeJobs = 0;
        for (const auto& [jobKey, existing] : m_audioPitchAnalysisJobs) {
            (void)jobKey;
            const auto status = existing->status.load(std::memory_order_acquire);
            if (status == 1 || status == 2) ++activeJobs;
        }
        const auto existing = m_audioPitchAnalysisJobs.find(key);
        if (activeJobs >= 2 || (existing != m_audioPitchAnalysisJobs.end() &&
            (existing->second->status.load(std::memory_order_acquire) == 1 ||
             existing->second->status.load(std::memory_order_acquire) == 2))) {
            target->finishAudioPitchAnalysis();
            return false;
        }
        job->status.store(1, std::memory_order_release);
        m_audioPitchAnalysisJobs[key] = job;
    }
    try {
        std::thread([target, job, rid, sampleRate]() {
            bool success = false;
            try {
                success = target->analyzeAudioNoteSegments(rid, sampleRate) &&
                    target->copyAudioNoteSegments(rid, job->after);
            } catch (...) {
                success = false;
            }
            target->finishAudioPitchAnalysis();
            job->status.store(success ? 2 : 4, std::memory_order_release);
        }).detach();
    } catch (...) {
        target->finishAudioPitchAnalysis();
        job->status.store(4, std::memory_order_release);
        return false;
    }
    return true;
}

uint8_t HirariUnifiedEngine::region_audio_note_analysis_status(uint32_t tid, uint32_t rid) const {
    const uint64_t key = (static_cast<uint64_t>(tid) << 32) | rid;
    std::lock_guard<std::mutex> lock(m_audioPitchAnalysisMutex);
    const auto job = m_audioPitchAnalysisJobs.find(key);
    return job == m_audioPitchAnalysisJobs.end()
        ? 0
        : job->second->status.load(std::memory_order_acquire);
}

bool HirariUnifiedEngine::finalize_region_audio_note_analysis(uint32_t tid, uint32_t rid) {
    const uint64_t key = (static_cast<uint64_t>(tid) << 32) | rid;
    std::shared_ptr<AudioPitchAnalysisJob> job;
    {
        std::lock_guard<std::mutex> lock(m_audioPitchAnalysisMutex);
        const auto found = m_audioPitchAnalysisJobs.find(key);
        if (found == m_audioPitchAnalysisJobs.end() ||
            found->second->status.load(std::memory_order_acquire) != 2) return false;
        job = found->second;
    }
    const auto target = job->target.lock();
    if (!target) {
        job->status.store(4, std::memory_order_release);
        return false;
    }
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        const auto current = std::find_if(m_tracks.begin(), m_tracks.end(),
            [tid](const auto& track) { return track->getId() == tid; });
        if (current == m_tracks.end() || *current != target) {
            job->status.store(4, std::memory_order_release);
            return false;
        }
    }
    const auto apply = [target, rid](const auto& segments) {
        (void)target->setAudioNoteSegments(rid, segments);
    };
    const auto actionName = "Analyze Audio Pitch " +
        std::to_string(tid) + ":" + std::to_string(rid);
    m_undoManager.recordAppliedAction(
        actionName, [apply, before = job->before] { apply(before); },
        [apply, after = job->after] { apply(after); });
    job->status.store(3, std::memory_order_release);
    return true;
}

bool HirariUnifiedEngine::set_region_loop_count(uint32_t tid, uint32_t rid, uint32_t count) {
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

bool HirariUnifiedEngine::set_region_locked(uint32_t tid, uint32_t rid, bool locked) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) if (track->getId() == tid) return track->setRegionLocked(rid, locked);
    return false;
}
bool HirariUnifiedEngine::set_region_sync_group(uint32_t tid, uint32_t rid, uint32_t group) {
    if (tid == 0 || rid == 0) return false;
    std::shared_ptr<Track> target;
    uint32_t before = 0;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (!track || track->getId() != tid) continue;
            const auto current = track->getRegionSyncGroup(rid);
            if (!current) return false;
            target = track;
            before = *current;
            break;
        }
    }
    if (!target) return false;
    auto apply = [target, rid](uint32_t value) { (void)target->setRegionSyncGroup(rid, value); };
    m_undoManager.performAction(
        "Region Sync Group", [apply, before] { apply(before); }, [apply, group] { apply(group); });
    return true;
}
bool HirariUnifiedEngine::set_region_warp_markers(
    uint32_t tid, uint32_t rid,
    const std::vector<::Hirari::Core::Engine::Region::WarpMarker>& markers) {
    if (tid == 0 || rid == 0 || markers.size() > 100'000) return false;
    std::shared_ptr<Track> target;
    std::vector<::Hirari::Core::Engine::Region::WarpMarker> before;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (!track || track->getId() != tid) continue;
            const auto current = track->getRegionWarpMarkers(rid);
            bool exists = false;
            for (const auto& region : track->getRegions()) {
                if (region.id == rid) { exists = true; break; }
            }
            if (!exists) return false;
            target = track;
            before = current;
            break;
        }
    }
    if (!target) return false;
    auto apply = [target, rid](const auto& value) {
        (void)target->setRegionWarpMarkers(rid, value);
    };
    if (!target->setRegionWarpMarkers(rid, markers)) return false;
    m_undoManager.recordAppliedAction(
        "Edit Region Warp Markers", [apply, before] { apply(before); },
        [apply, markers] { apply(markers); });
    return true;
}

bool HirariUnifiedEngine::align_regions_to_reference(
    uint32_t referenceTid, uint32_t referenceRid,
    const std::vector<std::pair<uint32_t, uint32_t>>& targets) {
    constexpr size_t kMaxTargets = 32;
    if (referenceTid == 0 || referenceRid == 0 || targets.empty() ||
        targets.size() > kMaxTargets) return false;

    auto job = std::make_shared<RegionAlignmentJob>();
    std::vector<std::pair<uint32_t, uint32_t>> uniqueTargets;
    uniqueTargets.reserve(targets.size());
    for (const auto& target : targets) {
        if (target.first == 0 || target.second == 0 ||
            (target.first == referenceTid && target.second == referenceRid) ||
            std::find(uniqueTargets.begin(), uniqueTargets.end(), target) != uniqueTargets.end())
            return false;
        uniqueTargets.push_back(target);
    }

    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks) {
            if (!track) continue;
            const auto regions = track->getRegions();
            for (const auto& region : regions) {
                if (track->getId() == referenceTid && region.id == referenceRid) {
                    job->referenceTrack = track;
                    job->referenceRegion = region;
                }
                for (const auto& targetId : uniqueTargets) {
                    if (track->getId() == targetId.first && region.id == targetId.second) {
                        auto it = std::find_if(job->targets.begin(), job->targets.end(),
                            [&targetId](const auto& candidate) {
                                return candidate.region.id == targetId.second &&
                                    candidate.track && candidate.track->getId() == targetId.first;
                            });
                        if (it == job->targets.end()) {
                            RegionAlignmentJob::Target target;
                            target.track = track;
                            target.region = region;
                            target.regionGeneration = track->regionGeneration();
                            target.before = track->getRegionWarpMarkers(region.id);
                            job->targets.push_back(std::move(target));
                        }
                    }
                }
            }
        }
        if (!job->referenceTrack || job->targets.size() != uniqueTargets.size()) return false;
        job->referenceRegionGeneration = job->referenceTrack->regionGeneration();
    }

    const auto validRegion = [](const Region& region, uint64_t expectedTimelineLength) {
        const uint64_t span = region.sourceLength > 0 ? region.sourceLength : region.len;
        constexpr uint64_t kMaxAlignmentSamples = 16u * 1024u * 1024u;
        return region.audio && !region.locked && !region.reverse && region.loopCount == 1 &&
            std::abs(region.warpRatio - 1.0) <= 1.0e-5 && span >= 1024 &&
            region.len == expectedTimelineLength && span <= kMaxAlignmentSamples &&
            region.sourceOffset <= region.audio->getNumSamples() &&
            span <= region.audio->getNumSamples() - region.sourceOffset &&
            region.audio->getNumChannels() > 0 && region.audio->getNumChannels() <= 32;
    };
    if (!validRegion(job->referenceRegion, job->referenceRegion.len) ||
        !job->referenceTrack->getRegionWarpMarkers(referenceRid).empty()) return false;
    for (const auto& target : job->targets) {
        if (!validRegion(target.region, job->referenceRegion.len)) return false;
    }

    const uint64_t key = (static_cast<uint64_t>(referenceTid) << 32) | referenceRid;
    {
        std::lock_guard<std::mutex> lock(m_regionAlignmentMutex);
        size_t active = 0;
        for (auto it = m_regionAlignmentJobs.begin(); it != m_regionAlignmentJobs.end();) {
            const auto status = it->second->status.load(std::memory_order_acquire);
            if (status == 3 || status == 4) {
                it = m_regionAlignmentJobs.erase(it);
                continue;
            }
            if (status == 1 || status == 2) {
                ++active;
                auto conflicts = [&job](const std::shared_ptr<Track>& track, uint32_t regionId) {
                    if (job->referenceTrack == track && job->referenceRegion.id == regionId) return true;
                    return std::any_of(job->targets.begin(), job->targets.end(),
                        [&track, regionId](const auto& target) {
                            return target.track == track && target.region.id == regionId;
                        });
                };
                if (it->first == key || conflicts(it->second->referenceTrack,
                        it->second->referenceRegion.id) ||
                    std::any_of(it->second->targets.begin(), it->second->targets.end(),
                        [&conflicts](const auto& target) { return conflicts(target.track, target.region.id); }))
                    return false;
            }
            ++it;
        }
        if (active >= 2) return false;
        m_regionAlignmentJobs[key] = job;
    }

    try {
        std::thread([job]() {
            try {
                const uint64_t referenceSpan = job->referenceRegion.sourceLength > 0
                    ? job->referenceRegion.sourceLength : job->referenceRegion.len;
                std::vector<const float*> referenceChannels(
                    job->referenceRegion.audio->getNumChannels(), nullptr);
                for (uint32_t channel = 0; channel < referenceChannels.size(); ++channel) {
                    referenceChannels[channel] =
                        job->referenceRegion.audio->getReadPointer(channel);
                }
                for (auto& target : job->targets) {
                    const uint64_t span = target.region.sourceLength > 0
                        ? target.region.sourceLength : target.region.len;
                    std::vector<const float*> targetChannels(
                        target.region.audio->getNumChannels(), nullptr);
                    for (uint32_t channel = 0; channel < targetChannels.size(); ++channel) {
                        targetChannels[channel] = target.region.audio->getReadPointer(channel);
                    }
                    std::array<::HirariWarpMarker, 257> markers{};
                    size_t markerCount = 0;
                    if (!hirari_region_align_audio(
                            referenceChannels.data(), referenceChannels.size(),
                            static_cast<size_t>(job->referenceRegion.sourceOffset),
                            static_cast<size_t>(referenceSpan),
                            targetChannels.data(), targetChannels.size(),
                            static_cast<size_t>(target.region.sourceOffset),
                            static_cast<size_t>(span), target.region.len,
                            markers.data(), markers.size(), &markerCount)) {
                        throw std::runtime_error("region alignment analysis failed");
                    }
                    target.after.clear();
                    target.after.reserve(markerCount);
                    for (size_t index = 0; index < markerCount; ++index) {
                        target.after.push_back({markers[index].sourceSample,
                            markers[index].timelineSample, markers[index].transient});
                    }
                }
                job->status.store(2, std::memory_order_release);
            } catch (...) { job->status.store(4, std::memory_order_release); }
        }).detach();
    } catch (...) {
        std::lock_guard<std::mutex> lock(m_regionAlignmentMutex);
        m_regionAlignmentJobs.erase(key);
        return false;
    }
    return true;
}

uint8_t HirariUnifiedEngine::region_alignment_status(uint32_t referenceTid, uint32_t referenceRid) const {
    const uint64_t key = (static_cast<uint64_t>(referenceTid) << 32) | referenceRid;
    std::lock_guard<std::mutex> lock(m_regionAlignmentMutex);
    const auto found = m_regionAlignmentJobs.find(key);
    return found == m_regionAlignmentJobs.end() ? 0
        : found->second->status.load(std::memory_order_acquire);
}

bool HirariUnifiedEngine::finalize_region_alignment(uint32_t referenceTid, uint32_t referenceRid) {
    const uint64_t key = (static_cast<uint64_t>(referenceTid) << 32) | referenceRid;
    std::shared_ptr<RegionAlignmentJob> job;
    {
        std::lock_guard<std::mutex> lock(m_regionAlignmentMutex);
        const auto found = m_regionAlignmentJobs.find(key);
        if (found == m_regionAlignmentJobs.end() ||
            found->second->status.load(std::memory_order_acquire) != 2) return false;
        job = found->second;
    }
    if (m_undoManager.transactionActive()) return false;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        if (std::find(m_tracks.begin(), m_tracks.end(), job->referenceTrack) == m_tracks.end() ||
            job->referenceTrack->regionGeneration() != job->referenceRegionGeneration ||
            std::any_of(job->targets.begin(), job->targets.end(), [this](const auto& target) {
                return std::find(m_tracks.begin(), m_tracks.end(), target.track) == m_tracks.end() ||
                    target.track->regionGeneration() != target.regionGeneration;
            })) {
            job->status.store(4, std::memory_order_release);
            return false;
        }
    }
    size_t applied = 0;
    for (; applied < job->targets.size(); ++applied) {
        auto& target = job->targets[applied];
        if (!target.track->setRegionWarpMarkers(target.region.id, target.after)) break;
    }
    if (applied != job->targets.size()) {
        while (applied > 0) {
            auto& target = job->targets[--applied];
            (void)target.track->setRegionWarpMarkers(target.region.id, target.before);
        }
        job->status.store(4, std::memory_order_release);
        return false;
    }
    m_undoManager.beginTransaction("Align Audio Regions to Reference");
    for (const auto& target : job->targets) {
        auto track = target.track;
        const auto targetId = target.region.id;
        const auto before = target.before;
        const auto after = target.after;
        auto apply = [track, targetId](const auto& markers) {
            (void)track->setRegionWarpMarkers(targetId, markers);
        };
        m_undoManager.recordAppliedAction("Align Audio Region to Reference",
            [apply, before] { apply(before); }, [apply, after] { apply(after); });
    }
    const bool committed = m_undoManager.endTransaction();
    if (!committed) for (const auto& target : job->targets)
        (void)target.track->setRegionWarpMarkers(target.region.id, target.before);
    job->status.store(committed ? 3 : 4, std::memory_order_release);
    return committed;
}

bool HirariUnifiedEngine::quantize_region_sync_group(
    uint32_t tid, uint32_t rid, float strength, double gridBeats, float swing) {
    constexpr std::array<double, 13> supportedGrids{
        1.0, 0.5, 0.25, 0.125, 0.0625,
        2.0 / 3.0, 1.0 / 3.0, 1.0 / 6.0, 1.0 / 12.0,
        1.5, 0.75, 0.375, 0.1875};
    if (tid == 0 || rid == 0 || !std::isfinite(strength) || strength < 0.0f || strength > 1.0f ||
        !std::isfinite(gridBeats) ||
        std::none_of(supportedGrids.begin(), supportedGrids.end(), [gridBeats](double grid) {
            return std::abs(grid - gridBeats) < 1.0e-9;
        }) || !std::isfinite(swing) || swing < -1.0f || swing > 1.0f) return false;

    std::vector<RegionQuantizeTarget> targets;
    Region sourceRegion;
    bool foundSource = false;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks) {
            if (!track) continue;
            for (const auto& region : track->getRegions()) {
                if (track->getId() == tid && region.id == rid) {
                    sourceRegion = region;
                    foundSource = true;
                }
            }
        }
        if (!foundSource || !sourceRegion.audio || sourceRegion.locked ||
            sourceRegion.reverse || sourceRegion.loopCount != 1 ||
            std::abs(sourceRegion.warpRatio - 1.0) > 1.0e-5) return false;
        const uint32_t group = sourceRegion.syncGroup;
        for (const auto& track : m_tracks) {
            if (!track) continue;
            for (const auto& region : track->getRegions()) {
                const bool selected = track->getId() == tid && region.id == rid;
                if (!selected && (group == 0 || region.syncGroup != group)) continue;
                if (!region.audio) continue;
                if (region.locked || region.reverse || region.loopCount != 1 ||
                    std::abs(region.warpRatio - 1.0) > 1.0e-5 ||
                    region.start != sourceRegion.start ||
                    region.len != sourceRegion.len) return false;
                targets.push_back({track, track->getRegionSnapshot(), region,
                    track->getRegionWarpMarkers(region.id)});
            }
        }
    }
    const uint64_t timelineSpan = sourceRegion.len;
    if (targets.empty() || timelineSpan < 2 ||
        timelineSpan > AudioQuantizer::kMaxWarpSamples ||
        sourceRegion.sourceOffset > sourceRegion.audio->getNumSamples() ||
        (sourceRegion.sourceLength > 0 ? sourceRegion.sourceLength : sourceRegion.len) >
            sourceRegion.audio->getNumSamples() - sourceRegion.sourceOffset ||
        !std::isfinite(sourceRegion.sourceFramesPerTimelineFrame()) ||
        sourceRegion.sourceFramesPerTimelineFrame() <= 0.0) return false;

    size_t totalChannels = 0;
    for (const auto& target : targets) {
        const uint32_t regionChannels = target.region.audio->getNumChannels();
        const uint64_t sourceSpan = target.region.sourceLength > 0
            ? target.region.sourceLength : target.region.len;
        if (sourceSpan < 2 || regionChannels == 0 || regionChannels > 32 - totalChannels ||
            target.region.sourceOffset > target.region.audio->getNumSamples() ||
            sourceSpan > target.region.audio->getNumSamples() - target.region.sourceOffset ||
            !std::isfinite(target.region.sourceFramesPerTimelineFrame()) ||
            target.region.sourceFramesPerTimelineFrame() <= 0.0) return false;
        totalChannels += regionChannels;
    }
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    const double baseTempo = m_tempoMap.currentBpm();
    const auto tempoEvents = m_tempoMap.getEvents();
    const uint64_t key = (static_cast<uint64_t>(tid) << 32) | rid;
    auto job = std::make_shared<RegionQuantizeJob>();
    job->targets = std::move(targets);
    job->tempoEvents = tempoEvents;
    job->baseTempo = baseTempo;
    job->sampleRate = sampleRate;
    {
        std::lock_guard<std::mutex> lock(m_regionQuantizeMutex);
        for (auto it = m_regionQuantizeJobs.begin(); it != m_regionQuantizeJobs.end();) {
            const auto status = it->second->status.load(std::memory_order_acquire);
            if (status == 3 || status == 4) {
                it = m_regionQuantizeJobs.erase(it);
                continue;
            }
            if (status == 1 || status == 2) return false;
            ++it;
        }
        job->status.store(1, std::memory_order_release);
        m_regionQuantizeJobs[key] = job;
    }
    try {
        std::thread([job, timelineSpan, sampleRate, baseTempo, tempoEvents,
                     gridBeats, strength, swing]() {
            try {
                const auto selected = std::find_if(job->targets.begin(), job->targets.end(),
                    [](const auto& target) { return target.region.id != 0; });
                if (selected == job->targets.end()) throw std::runtime_error("no quantize source");
                const uint64_t timelineStart = selected->region.start;
                std::vector<::HirariAudioQuantizeChannelView> channels;
                channels.reserve(32);
                for (const auto& target : job->targets) {
                    const uint32_t regionChannels = target.region.audio->getNumChannels();
                    const uint64_t sourceSpan = target.region.sourceLength > 0
                        ? target.region.sourceLength : target.region.len;
                    const double sourceRate = target.region.sourceFramesPerTimelineFrame();
                    for (uint32_t channel = 0; channel < regionChannels; ++channel) {
                        const float* data = target.region.audio->getReadPointer(channel);
                        if (!data) throw std::runtime_error("audio channel unavailable");
                        channels.push_back({data + target.region.sourceOffset, sourceSpan, sourceRate});
                    }
                }
                // Analyze every microphone at the project timeline rate. A
                // compact RMS envelope keeps the working set bounded while
                // mapping the resulting anchors back to each native source.
                std::vector<float> groupEnvelope(static_cast<size_t>(timelineSpan));
                if (!hirari_audio_quantizer_build_group_envelope(
                        channels.data(), channels.size(), timelineSpan,
                        groupEnvelope.data(), groupEnvelope.size()))
                    throw std::runtime_error("audio group envelope could not be built");
                TempoMap tempoSnapshot;
                tempoSnapshot.clear(sampleRate, baseTempo);
                for (const auto& event : tempoEvents)
                    tempoSnapshot.addTempo(event.samplePos, event.bpm, sampleRate, event.ramp);
                const auto samplesToBeats = [&tempoSnapshot, sampleRate](uint64_t sample) {
                    return tempoSnapshot.samplesToBeats(sample, sampleRate);
                };
                const auto beatsToSamples = [&tempoSnapshot, sampleRate](double beat) {
                    return tempoSnapshot.beatsToSamples(beat, sampleRate);
                };
                const float* analysisInputs[] = {groupEnvelope.data()};
                std::vector<std::pair<uint64_t, uint64_t>> map;
                if (!AudioQuantizer::buildGroupWarpMap(analysisInputs, 1,
                        timelineSpan, timelineStart,
                        gridBeats, {strength, swing}, samplesToBeats, beatsToSamples, map))
                    throw std::runtime_error("transient map could not be built");
                std::vector<::HirariAudioQuantizerMapPoint> nativeMap;
                nativeMap.reserve(map.size());
                for (const auto& point : map) nativeMap.push_back({point.first, point.second});
                job->after.resize(job->targets.size());
                for (size_t targetIndex = 0; targetIndex < job->targets.size(); ++targetIndex) {
                    const auto& region = job->targets[targetIndex].region;
                    const uint64_t sourceSpan = region.sourceLength > 0
                        ? region.sourceLength : region.len;
                    const double sourceRate = region.sourceFramesPerTimelineFrame();
                    std::vector<::HirariWarpMarker> projected(nativeMap.size() + 2);
                    size_t projectedCount = 0;
                    if (!hirari_audio_quantizer_project_group_map(
                            nativeMap.data(), nativeMap.size(), sourceSpan, timelineSpan,
                            sourceRate, projected.data(), projected.size(), &projectedCount))
                        throw std::runtime_error("group warp map projection failed");
                    auto& markers = job->after[targetIndex];
                    markers.clear();
                    markers.reserve(projectedCount);
                    for (size_t index = 0; index < projectedCount; ++index) {
                        markers.push_back({projected[index].sourceSample,
                            projected[index].timelineSample, projected[index].transient});
                    }
                }
                job->status.store(2, std::memory_order_release);
            } catch (...) {
                job->status.store(4, std::memory_order_release);
            }
        }).detach();
    } catch (...) {
        job->status.store(4, std::memory_order_release);
        return false;
    }
    return true;
}

uint8_t HirariUnifiedEngine::region_quantize_status(uint32_t tid, uint32_t rid) const {
    const uint64_t key = (static_cast<uint64_t>(tid) << 32) | rid;
    std::lock_guard<std::mutex> lock(m_regionQuantizeMutex);
    const auto found = m_regionQuantizeJobs.find(key);
    return found == m_regionQuantizeJobs.end()
        ? 0 : found->second->status.load(std::memory_order_acquire);
}

bool HirariUnifiedEngine::finalize_region_quantize(uint32_t tid, uint32_t rid) {
    const uint64_t key = (static_cast<uint64_t>(tid) << 32) | rid;
    std::shared_ptr<RegionQuantizeJob> job;
    {
        std::lock_guard<std::mutex> lock(m_regionQuantizeMutex);
        const auto found = m_regionQuantizeJobs.find(key);
        if (found == m_regionQuantizeJobs.end() ||
            found->second->status.load(std::memory_order_acquire) != 2) return false;
        job = found->second;
    }
    // Preserve another active UI gesture's transaction. The caller polls and
    // retries once that gesture commits or aborts.
    if (m_undoManager.transactionActive()) return false;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& target : job->targets) {
            if (std::find(m_tracks.begin(), m_tracks.end(), target.track) == m_tracks.end() ||
                target.track->getRegionSnapshot() != target.snapshot) {
                job->status.store(4, std::memory_order_release);
                return false;
            }
        }
    }
    const auto currentTempoEvents = m_tempoMap.getEvents();
    if (std::abs(m_tempoMap.currentBpm() - job->baseTempo) > 1.0e-9 ||
        std::abs(m_sampleRate.load(std::memory_order_relaxed) - job->sampleRate) > 1.0e-9 ||
        currentTempoEvents.size() != job->tempoEvents.size() ||
        !std::equal(currentTempoEvents.begin(), currentTempoEvents.end(), job->tempoEvents.begin(),
            [](const auto& left, const auto& right) {
                return left.samplePos == right.samplePos && left.bpm == right.bpm &&
                    left.ramp == right.ramp;
            })) {
        job->status.store(4, std::memory_order_release);
        return false;
    }
    size_t applied = 0;
    for (; applied < job->targets.size(); ++applied) {
        const auto& target = job->targets[applied];
        if (applied >= job->after.size() ||
            !target.track->setRegionWarpMarkers(target.region.id, job->after[applied])) break;
    }
    if (applied != job->targets.size()) {
        while (applied > 0) {
            --applied;
            const auto& target = job->targets[applied];
            (void)target.track->setRegionWarpMarkers(target.region.id, target.before);
        }
        job->status.store(4, std::memory_order_release);
        return false;
    }
    m_undoManager.beginTransaction("Quantize Synchronized Audio Regions");
    for (size_t targetIndex = 0; targetIndex < job->targets.size(); ++targetIndex) {
        const auto& target = job->targets[targetIndex];
        auto track = target.track;
        const uint32_t regionId = target.region.id;
        const auto before = target.before;
        const auto after = job->after[targetIndex];
        auto apply = [track, regionId](const auto& value) {
            (void)track->setRegionWarpMarkers(regionId, value);
        };
        m_undoManager.recordAppliedAction("Quantize Synchronized Audio Regions",
            [apply, before] { apply(before); }, [apply, after] { apply(after); });
    }
    const bool committed = m_undoManager.endTransaction();
    job->status.store(committed ? 3 : 4, std::memory_order_release);
    return committed;
}
bool HirariUnifiedEngine::append_region_processing(uint32_t tid, uint32_t rid, uint32_t stepId, std::string_view operation, float parameter, bool enabled) {
    if (stepId == 0 || operation.empty() || operation.size() > 256 || !std::isfinite(parameter)) return false;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) if (track->getId() == tid)
        return track->appendProcessingStep(rid, {stepId, std::string(operation), parameter, enabled});
    return false;
}
bool HirariUnifiedEngine::clear_region_processing(uint32_t tid, uint32_t rid) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) if (track->getId() == tid) return track->clearProcessingHistory(rid);
    return false;
}

bool HirariUnifiedEngine::loadProject(std::string_view path) {
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
        std::unordered_set<uint32_t> busTrackIds;
        trackIds.reserve(state.tracks.size());
        busTrackIds.reserve(state.tracks.size());
        for (const auto& ts : state.tracks) {
            if (ts.id == std::numeric_limits<uint32_t>::max() ||
                std::find(trackIds.begin(), trackIds.end(), ts.id) != trackIds.end()) return false;
            trackIds.push_back(ts.id);
            if (ts.type == static_cast<uint32_t>(Track::Bus)) busTrackIds.insert(ts.id);
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
        Hirari::Core::Engine::RoutingEngine replacementRouting;
        std::unordered_set<uint64_t> serializedRouteKeys;
        for (const auto& route : state.routes) {
            if (route.sourceId == route.destinationId ||
                route.sourceId >= Hirari::Core::Engine::RoutingEngine::kMaxNodes ||
                route.destinationId >= Hirari::Core::Engine::RoutingEngine::kMaxNodes ||
                !std::isfinite(route.gain) || route.gain <= 0.0f || route.gain > 2.0f ||
                std::find(trackIds.begin(), trackIds.end(), route.sourceId) == trackIds.end() ||
                std::find(trackIds.begin(), trackIds.end(), route.destinationId) == trackIds.end() ||
                (route.send && !busTrackIds.contains(route.destinationId))) {
                return false;
            }
            const uint64_t routeKey = ((static_cast<uint64_t>(route.sourceId) << 32) |
                                       static_cast<uint64_t>(route.destinationId)) ^
                                      (route.send ? 0x8000000000000000ull : 0ull);
            if (!serializedRouteKeys.insert(routeKey).second) return false;
            if (!route.send && route.preFader) return false;
            replacementRouting.addConnection(route.sourceId, route.destinationId, route.gain,
                                             route.send, route.preFader);
        }
        std::vector<Hirari::Core::Engine::RoutingEngine::Connection> validatedRoutes;
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
                    &m_sidechainManager, m_projectLayoutRevision);
                // Do not mutate the process/session BusSystem while the
                // replacement document is still provisional. If hydration
                // later fails, the old project must remain the only visible
                // routing state. Resolve the bus after the commit point.
                t = std::move(busTrack);
            } else {
                t = std::make_shared<Track>(ts.id,
                    ts.pluginName.empty() ? "Track" : ts.pluginName, trackType,
                    &m_sidechainManager, m_projectLayoutRevision);
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
            for (const auto& lane : ts.pluginAutomation) {
                if (lane.pluginIndex >= t->getPluginTypes().size()) return false;
            }
            if (!ts.pluginAutomation.empty()) t->setPluginAutomation(ts.pluginAutomation);
            for (size_t stateIndex = 0; stateIndex < ts.pluginStates.size(); ++stateIndex) {
