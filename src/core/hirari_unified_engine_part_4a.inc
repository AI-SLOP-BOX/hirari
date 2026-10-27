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

bool HirariUnifiedEngine::set_region_reverse(uint32_t tid, uint32_t rid, bool reverse) {
    return hirari_region_edit_scalar(this, tid, rid, kHirariRegionScalarReverse,
                                     reverse ? 1.0f : 0.0f, 0.0f);
}

bool HirariUnifiedEngine::set_region_trim(uint32_t tid, uint32_t rid, float startNorm, float endNorm) {
    return hirari_region_edit_trim(this, tid, rid, startNorm, endNorm);
}

bool HirariUnifiedEngine::set_region_warp_ratio(uint32_t tid, uint32_t rid, double ratio) {
    return hirari_region_edit_warp_ratio(this, tid, rid, ratio);
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

// Pitch analysis is a Rust job (region_jobs.rs): rate validation, the worker,
// its registry limits, the staleness check and the undo pair all live there.
// The engine only exposes the retained-track storage callbacks (part_4z).
bool HirariUnifiedEngine::start_region_audio_note_analysis(
    uint32_t tid, uint32_t rid, double sampleRate) {
    return hirari_region_pitch_analysis_start(this, tid, rid, sampleRate);
}

uint8_t HirariUnifiedEngine::region_audio_note_analysis_status(uint32_t tid,
                                                              uint32_t rid) const {
    return hirari_region_pitch_analysis_status(
        const_cast<HirariUnifiedEngine*>(this), tid, rid);
}

bool HirariUnifiedEngine::finalize_region_audio_note_analysis(uint32_t tid, uint32_t rid) {
    return hirari_region_pitch_analysis_finalize(this, tid, rid);
}

bool HirariUnifiedEngine::set_region_loop_count(uint32_t tid, uint32_t rid, uint32_t count) {
    return hirari_region_edit_scalar(this, tid, rid, kHirariRegionScalarLoopCount,
                                     std::bit_cast<float>(count), 0.0f);
}

bool HirariUnifiedEngine::set_region_sync_group(uint32_t tid, uint32_t rid, uint32_t group) {
    return hirari_region_edit_scalar(this, tid, rid, kHirariRegionScalarSyncGroup,
                                     std::bit_cast<float>(group), 0.0f);
}
// Alignment and sync-group quantize are Rust jobs: validation, the job
// registry, analysis threads, staleness checks and the marker commit all live
// in hirari-core-bridge/src/region_time_jobs.rs. The engine only provides the
// host callbacks (part_4z) that expose storage.
bool HirariUnifiedEngine::align_regions_to_reference(
    uint32_t referenceTid, uint32_t referenceRid,
    const std::vector<::HirariRegionAlignmentTargetId>& targets) {
    return hirari_region_align_to_reference(this, referenceTid, referenceRid,
                                            targets.data(), targets.size());
}

uint8_t HirariUnifiedEngine::region_alignment_status(uint32_t referenceTid,
                                                     uint32_t referenceRid) const {
    return hirari_region_alignment_status(
        const_cast<HirariUnifiedEngine*>(this), referenceTid, referenceRid);
}

bool HirariUnifiedEngine::finalize_region_alignment(uint32_t referenceTid,
                                                    uint32_t referenceRid) {
    return hirari_region_finalize_alignment(this, referenceTid, referenceRid);
}

bool HirariUnifiedEngine::quantize_region_sync_group(
    uint32_t tid, uint32_t rid, float strength, double gridBeats, float swing) {
    return hirari_region_quantize_sync_group(this, tid, rid, strength, gridBeats, swing);
}

uint8_t HirariUnifiedEngine::region_quantize_status(uint32_t tid, uint32_t rid) const {
    return hirari_region_quantize_status(const_cast<HirariUnifiedEngine*>(this), tid, rid);
}

bool HirariUnifiedEngine::finalize_region_quantize(uint32_t tid, uint32_t rid) {
    return hirari_region_finalize_quantize(this, tid, rid);
}
bool HirariUnifiedEngine::append_region_processing(uint32_t tid, uint32_t rid, uint32_t stepId, std::string_view operation, float parameter, bool enabled) {
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
    if (state.valid) {
        const double projectSampleRate = static_cast<double>(state.sampleRate);
        const uint32_t configuredBlockSize = m_context ? m_context->blockSize : 0u;
        // A freshly created bridge may not have an audio device configured
        // yet. Project hydration still needs prepared track buffers so that
        // routing and sidechain links can be rebound deterministically.
        const uint32_t currentBlockSize = configuredBlockSize == 0
            ? 512u
            : configuredBlockSize;
        // Every document-level invariant (track/region ids, references,
        // routing graph, sidechain cycles, plugin-array shape) is enforced by
        // native_project_load, which only returns a valid state when the file
        // is safe to hydrate.
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
            // Plugin arrays are positional within a serialized track. The
            // codec rejected inconsistent counts before this state was marked
            // valid, so the chain can be rebuilt positionally here.
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
                    // The codec guarantees one sandbox path per marker. An
                    // unavailable binary is quarantined; the path is still
                    // consumed so the following plugin keeps its position.
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
