bool HirariUnifiedEngine::move_region(uint32_t tid, uint32_t rid, double startBeat) {
    return hirari_region_edit_move(this, tid, rid, startBeat);
}

bool HirariUnifiedEngine::move_region_sync_group(uint32_t tid, uint32_t rid, double startSample) {
    return hirari_region_edit_move_sync_group(this, tid, rid, startSample);
}

bool HirariUnifiedEngine::remove_region(uint32_t tid, uint32_t rid) {
    return hirari_region_edit_remove(this, tid, rid);
}

bool HirariUnifiedEngine::set_region_gain(uint32_t tid, uint32_t rid, float gain) {
    return hirari_region_edit_scalar(this, tid, rid, kHirariRegionScalarClipGain, gain, 0.0f);
}

bool HirariUnifiedEngine::set_region_muted(uint32_t tid, uint32_t rid, bool muted) {
    return hirari_region_edit_scalar(this, tid, rid, kHirariRegionScalarMuted,
                                     muted ? 1.0f : 0.0f, 0.0f);
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
    return hirari_region_edit_scalar(this, tid, rid, kHirariRegionScalarFades, fadeIn, fadeOut);
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
    return hirari_region_edit_scalar(this, tid, rid, kHirariRegionScalarReverse,
                                     reverse ? 1.0f : 0.0f, 0.0f);
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

bool HirariUnifiedEngine::set_region_pitch_preserve_warp(uint32_t tid, uint32_t rid,
                                                        bool enabled) {
    return hirari_region_edit_scalar(this, tid, rid, kHirariRegionScalarPitchPreserveWarp,
                                     enabled ? 1.0f : 0.0f, 0.0f);
}

bool HirariUnifiedEngine::set_region_pitch_semitones(uint32_t tid, uint32_t rid, float semitones) {
    return hirari_region_edit_scalar(this, tid, rid, kHirariRegionScalarPitchSemitones, semitones,
                                     0.0f);
}

bool HirariUnifiedEngine::set_region_audio_note_segment(
    uint32_t tid, uint32_t rid, double startSeconds, double endSeconds,
    double pitchOffsetCents, double formantOffsetCents) {
    return hirari_region_edit_note_segment(this, tid, rid, startSeconds, endSeconds,
                                           pitchOffsetCents, formantOffsetCents);
}

bool HirariUnifiedEngine::set_region_audio_note_anchor(
    uint32_t tid, uint32_t rid, double segmentStartSeconds,
    double positionSeconds, double pitchCents, double formantCents) {
    return hirari_region_edit_note_anchor(this, tid, rid, segmentStartSeconds,
                                          positionSeconds, pitchCents, formantCents);
}

bool HirariUnifiedEngine::set_region_audio_note_formant_anchor(
    uint32_t tid, uint32_t rid, double segmentStartSeconds,
    double positionSeconds, double formantCents) {
    return hirari_region_edit_note_formant_anchor(this, tid, rid, segmentStartSeconds,
                                                  positionSeconds, formantCents);
}

bool HirariUnifiedEngine::set_region_audio_note_pitch_anchor(
    uint32_t tid, uint32_t rid, double segmentStartSeconds,
    double positionSeconds, double pitchCents) {
    return hirari_region_edit_note_pitch_anchor(this, tid, rid, segmentStartSeconds,
                                                positionSeconds, pitchCents);
}

bool HirariUnifiedEngine::move_region_audio_note_anchor(
    uint32_t tid, uint32_t rid, double segmentStartSeconds,
    double oldPositionSeconds, double newPositionSeconds,
    double valueCents, bool editFormant) {
    return hirari_region_edit_note_move_anchor(this, tid, rid, segmentStartSeconds,
                                               oldPositionSeconds, newPositionSeconds,
                                               valueCents, editFormant);
}

bool HirariUnifiedEngine::clear_region_audio_note_segments(uint32_t tid, uint32_t rid) {
    return hirari_region_edit_note_clear(this, tid, rid);
}

bool HirariUnifiedEngine::warp_region_audio_note_segment(
    uint32_t tid, uint32_t rid, double segmentStartSeconds,
    double newStartSeconds, double newEndSeconds) {
    return hirari_region_edit_note_warp(this, tid, rid, segmentStartSeconds,
                                        newStartSeconds, newEndSeconds);
}

bool HirariUnifiedEngine::remove_region_audio_note_segment(uint32_t tid, uint32_t rid,
                                                          double segmentStartSeconds) {
    return hirari_region_edit_note_remove(this, tid, rid, segmentStartSeconds);
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
    return hirari_region_edit_scalar(this, tid, rid, kHirariRegionScalarLoopCount,
                                     std::bit_cast<float>(count), 0.0f);
}

bool HirariUnifiedEngine::set_region_sync_group(uint32_t tid, uint32_t rid, uint32_t group) {
    return hirari_region_edit_scalar(this, tid, rid, kHirariRegionScalarSyncGroup,
                                     std::bit_cast<float>(group), 0.0f);
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
    const std::vector<::HirariRegionAlignmentTargetId>& targets) {
    if (targets.empty() || targets.size() > 32) return false;
    if (!hirari_region_alignment_target_ids_valid(referenceTid, referenceRid,
            targets.data(), targets.size())) return false;

    auto job = std::make_shared<RegionAlignmentJob>();

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
                for (const auto& targetId : targets) {
                    if (track->getId() == targetId.track_id && region.id == targetId.region_id) {
                        auto it = std::find_if(job->targets.begin(), job->targets.end(),
                            [&targetId](const auto& candidate) {
                                return candidate.region.id == targetId.region_id &&
                                    candidate.track && candidate.track->getId() == targetId.track_id;
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
        if (!job->referenceTrack || job->targets.size() != targets.size()) return false;
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
                std::vector<const float*> targetChannels;
                std::vector<::HirariRegionAlignmentTargetView> targetViews;
                targetViews.reserve(job->targets.size());
                for (auto& target : job->targets) {
                    const uint64_t span = target.region.sourceLength > 0
                        ? target.region.sourceLength : target.region.len;
                    const size_t channelOffset = targetChannels.size();
                    const uint32_t channelCount = target.region.audio->getNumChannels();
                    for (uint32_t channel = 0; channel < channelCount; ++channel) {
                        targetChannels.push_back(
                            target.region.audio->getReadPointer(channel));
                    }
                    targetViews.push_back({channelOffset, channelCount,
                        static_cast<size_t>(target.region.sourceOffset),
                        static_cast<size_t>(span), target.region.len});
                }
                std::unique_ptr<void, decltype(&hirari_region_alignment_group_maps_destroy)>
                    maps{hirari_region_align_audio_group(
                        referenceChannels.data(), referenceChannels.size(),
                        static_cast<size_t>(job->referenceRegion.sourceOffset),
                        static_cast<size_t>(referenceSpan), targetChannels.data(),
                        targetChannels.size(), targetViews.data(), targetViews.size()),
                        &hirari_region_alignment_group_maps_destroy};
                if (!maps) throw std::runtime_error("region alignment analysis failed");
                for (size_t index = 0; index < job->targets.size(); ++index) {
                    std::array<::HirariWarpMarker, 257> markers{};
                    const size_t markerCount = hirari_region_alignment_group_map_copy(
                        maps.get(), index, markers.data(), markers.size());
                    if (markerCount < 2) throw std::runtime_error("region alignment map is empty");
                    job->targets[index].after.assign(markers.begin(), markers.begin() + markerCount);
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
    if (!hirari_audio_quantizer_settings_validate(tid, rid, strength, gridBeats, swing))
        return false;

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
        timelineSpan > hirari::limits::kMaxAudioQuantizerWarpSamples ||
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
    const double baseTempo = hirari_tempo_map_get_current_bpm(m_tempoState);
    const auto tempoEvents = readTempoEvents(m_tempoState);
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
                std::vector<::HirariAudioQuantizerTargetView> targetViews;
                targetViews.reserve(job->targets.size());
                for (const auto& target : job->targets) {
                    targetViews.push_back({
                        target.region.sourceLength > 0
                            ? target.region.sourceLength : target.region.len,
                        target.region.sourceFramesPerTimelineFrame()});
                }
                std::unique_ptr<void, decltype(&hirari_audio_quantizer_group_warp_maps_destroy)>
                    projected{hirari_audio_quantizer_build_group_warp_maps(
                        channels.data(), channels.size(), targetViews.data(), targetViews.size(),
                        timelineSpan, timelineStart, gridBeats, strength, swing, sampleRate,
                        baseTempo, tempoEvents.data(), tempoEvents.size()),
                        &hirari_audio_quantizer_group_warp_maps_destroy};
                if (!projected) throw std::runtime_error("synchronized warp maps could not be built");
                job->after.resize(job->targets.size());
                for (size_t targetIndex = 0; targetIndex < job->targets.size(); ++targetIndex) {
                    const size_t projectedCount = hirari_audio_quantizer_group_warp_map_count(
                        projected.get(), targetIndex);
                    if (projectedCount < 2)
                        throw std::runtime_error("synchronized warp map is empty");
                    auto& markers = job->after[targetIndex];
                    markers.resize(projectedCount);
                    if (!hirari_audio_quantizer_group_warp_map_copy(
                            projected.get(), targetIndex, markers.data(), markers.size()))
                        throw std::runtime_error("synchronized warp map could not be copied");
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
    const auto currentTempoEvents = readTempoEvents(m_tempoState);
    if (std::abs(hirari_tempo_map_get_current_bpm(m_tempoState) - job->baseTempo) > 1.0e-9 ||
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
    auto state = ::Hirari::Core::Bridge::native_project_load(
        rust::Str(path.data(), path.size()));
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
            if (ts.trackType == static_cast<uint32_t>(Track::Bus)) busTrackIds.insert(ts.id);
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
        // The bridge state is Rust-owned, so the few engine setters that take
        // standard containers materialize a control-side copy here.
        const auto toAutomation = [](const rust::Vec<::Hirari::Core::Bridge::NativeAutomationPoint>& points) {
            std::vector<ProjectAutomationPoint> converted;
            converted.reserve(points.size());
            for (const auto& point : points) {
                converted.push_back({point.time, point.value, point.curve});
            }
            return converted;
        };
        const auto toBlob = [](const rust::Vec<uint8_t>& bytes) {
            return std::vector<uint8_t>(bytes.begin(), bytes.end());
        };
        const auto toPluginAutomation =
            [&toAutomation](const rust::Vec<::Hirari::Core::Bridge::NativePluginAutomationLane>& lanes) {
                std::vector<ProjectPluginAutomationLane> converted;
                converted.reserve(lanes.size());
                for (const auto& lane : lanes) {
                    ProjectPluginAutomationLane value;
                    value.pluginIndex = lane.pluginIndex;
                    value.parameterId = lane.parameterId;
                    value.points = toAutomation(lane.points);
                    converted.push_back(std::move(value));
                }
                return converted;
            };
        const auto readPluginType = [](const rust::Vec<uint8_t>& bytes, size_t offset) {
            return static_cast<uint32_t>(bytes[offset]) |
                   (static_cast<uint32_t>(bytes[offset + 1]) << 8) |
                   (static_cast<uint32_t>(bytes[offset + 2]) << 16) |
                   (static_cast<uint32_t>(bytes[offset + 3]) << 24);
        };

        // 1. Hydrate Tracks
        for (const auto& ts : state.tracks) {
            const auto trackType = static_cast<Track::Type>(ts.trackType);
            const std::string trackName = std::string(ts.pluginName);
            std::shared_ptr<Track> t;
            if (trackType == Track::Type::Bus) {
                auto busTrack = std::make_shared<BusTrack>(
                    ts.id, trackName.empty() ? "Bus" : trackName, ts.id,
                    &m_sidechainManager, m_projectLayoutRevision);
                // Do not mutate the process/session BusSystem while the
                // replacement document is still provisional. If hydration
                // later fails, the old project must remain the only visible
                // routing state. Resolve the bus after the commit point.
                t = std::move(busTrack);
            } else {
                t = std::make_shared<Track>(ts.id,
                    trackName.empty() ? "Track" : trackName, trackType,
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
            if (!ts.volumeAutomation.empty()) t->setVolumeAutomation(toAutomation(ts.volumeAutomation));
            if (!ts.panAutomation.empty()) t->setPanAutomation(toAutomation(ts.panAutomation));
            if (!ts.trackDelayAutomation.empty()) t->setTrackDelayAutomation(toAutomation(ts.trackDelayAutomation));
            // pluginData stores plugin type IDs as little-endian u32 values.
            // 0xffffffff is the serialized marker for a sandbox plugin. Build
            // the processor chain in the same order in which it was saved;
            // restoring states before creating sandbox processors makes a
            // valid project fail when the first state targets that processor.
            size_t sandboxPathIndex = 0;
            for (size_t offset = 0; offset + sizeof(uint32_t) <= ts.pluginData.size();
                 offset += sizeof(uint32_t)) {
                const uint32_t pluginType = readPluginType(ts.pluginData, offset);
                if (pluginType == 0xFFFFFFFFu) {
                    if (sandboxPathIndex >= ts.sandboxedPluginPaths.size()) return false;
                    // An unavailable external binary is represented as a
                    // quarantined/missing plugin and does not invalidate the
                    // rest of the project. The path is still consumed so the
                    // following serialized plugin keeps its position.
                    (void)t->addSandboxedPlugin(std::string(ts.sandboxedPluginPaths[sandboxPathIndex]));
                    ++sandboxPathIndex;
                } else if (!t->addPlugin(pluginType)) {
                    return false;
                }
            }
            // Older files may have sandbox paths without the marker in
            // pluginData. Preserve compatibility by appending those entries.
            while (sandboxPathIndex < ts.sandboxedPluginPaths.size()) {
                (void)t->addSandboxedPlugin(std::string(ts.sandboxedPluginPaths[sandboxPathIndex++]));
            }
            for (const auto& lane : ts.pluginAutomation) {
                if (lane.pluginIndex >= t->getPluginTypes().size()) return false;
            }
            if (!ts.pluginAutomation.empty()) t->setPluginAutomation(toPluginAutomation(ts.pluginAutomation));
            for (size_t stateIndex = 0; stateIndex < ts.pluginStates.size(); ++stateIndex) {
