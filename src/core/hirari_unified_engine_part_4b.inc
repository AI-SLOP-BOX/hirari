                if (stateIndex >= t->getPluginTypes().size()) break;
                // Missing sandbox binaries intentionally leave a hole in the
                // live chain; their state is retained in the serialized
                // document but cannot be applied until the plugin is relinked.
                (void)t->setPluginState(stateIndex, toBlob(ts.pluginStates[stateIndex].bytes));
            }
            for (size_t guiIndex = 0; guiIndex < ts.pluginGuiStates.size(); ++guiIndex) {
                if (guiIndex >= t->getPluginTypes().size()) break;
                (void)t->setPluginGuiState(guiIndex, toBlob(ts.pluginGuiStates[guiIndex].bytes));
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
                    const std::filesystem::path storedPath(std::string(rs.filePath));
                    const std::filesystem::path resolvedPath = storedPath.is_absolute()
                        ? storedPath
                        : projectDir / storedPath;
                    const std::string resolvedFilePath = resolvedPath.lexically_normal().string();
                    Region r;
                    if (state.version >= 28) {
                        r.id = rs.id;
                        nextRegionId = std::max(nextRegionId, r.id + 1u);
                    } else {
                        r.id = nextRegionId++;
                    }
                    r.path = resolvedFilePath;
                    r.start = rs.samplePosition;
                    r.len = rs.sampleLength;
                    r.sourceLength = rs.sourceLength;
                    r.sourceSampleRate = rs.sourceSampleRate > 0
                        ? rs.sourceSampleRate : projectSampleRate;
                    r.timelineSampleRate = projectSampleRate;
                    r.sourceOffset = rs.sourceOffset;
                    r.baseStart = rs.baseStart == 0 ? r.start : rs.baseStart;
                    r.baseSourceOffset = rs.baseSourceOffset;
                    r.baseLength = rs.baseLength == 0 ? r.len : rs.baseLength;
                    r.muted = rs.isMuted;
                    const std::string regionName(rs.name);
                    r.name = regionName.empty() ? resolvedFilePath : regionName;
                    r.clipGain = std::clamp(rs.clipGain, 0.0f, 2.0f);
                    r.fadeInSamples = std::min(rs.fadeInSamples, r.len);
                    r.fadeOutSamples = std::min(rs.fadeOutSamples, r.len);
                    r.reverse = rs.reverse;
                    r.warpRatio = rs.warpRatio;
                    r.pitchPreserveWarp = rs.pitchPreserveWarp;
                    r.pitchSemitones = rs.pitchSemitones;
                    r.loopCount = rs.loopCount;
                    r.locked = rs.locked;
                    r.syncGroup = rs.syncGroup;
                    r.warpMarkers.clear();
                    r.warpMarkers.reserve(rs.warpMarkers.size());
                    for (const auto& marker : rs.warpMarkers) {
                        r.warpMarkers.push_back({marker.sourceSample, marker.timelineSample,
                                                 marker.transient});
                    }
                    for (const auto& step : rs.processingHistory) {
                        ::hirari::editing::EventProcessingStep converted;
                        converted.id = step.id;
                        converted.operation = std::string(step.operation);
                        converted.parameter = step.parameter;
                        converted.enabled = step.enabled;
                        if (!r.processingHistory.append(converted)) break;
                    }
                    std::vector<::hirari::editing::AudioNoteSegment> noteSegments;
                    noteSegments.reserve(rs.audioNoteSegments.size());
                    for (const auto& segment : rs.audioNoteSegments) {
                        ::hirari::editing::AudioNoteSegment converted;
                        converted.startSeconds = segment.startSeconds;
                        converted.endSeconds = segment.endSeconds;
                        converted.detectedPitchCents = segment.detectedPitchCents;
                        converted.pitchOffsetCents = segment.pitchOffsetCents;
                        converted.formantOffsetCents = segment.formantOffsetCents;
                        converted.anchors.reserve(segment.anchors.size());
                        for (const auto& anchor : segment.anchors) {
                            converted.anchors.push_back({anchor.positionSeconds,
                                                         anchor.pitchCents,
                                                         anchor.formantCents});
                        }
                        noteSegments.push_back(std::move(converted));
                    }
                    if (!r.noteEdits.assignRepaired(noteSegments)) return false;
                    r.rangeEdits.clear();
                    r.rangeEdits.reserve(std::min<size_t>(rs.rangeEdits.size(), 4096));
                    for (const auto& edit : rs.rangeEdits) {
                        if (r.rangeEdits.size() >= 4096) break;
                        r.rangeEdits.push_back({edit.start, edit.end, edit.gain, edit.fadeIn, edit.fadeOut});
                    }
                    // Rust rejects malformed persisted edits before they reach
                    // the real-time renderer.
                    r.rangeEdits.resize(hirari_region_range_edits_sanitize(
                        reinterpret_cast<HirariLegacyRangeEdit*>(r.rangeEdits.data()),
                        r.rangeEdits.size(), r.len));
                    // Hydrate audio during project load so an opened project
                    // is immediately playable. Missing files remain visible
                    // as unloaded regions and can be repaired via Relink.
                    std::error_code fileError;
                    if (std::filesystem::is_regular_file(resolvedPath, fileError) && !fileError) {
                        double decodedSourceRate = 0.0;
                        auto audio = m_audioDecoderManager.importFile(
                            resolvedFilePath, &decodedSourceRate);
                        if (audio && audio->getNumSamples() > 0 && audio->getNumChannels() > 0) {
                            r.audio = std::move(audio);
                            r.timelineSampleRate = projectSampleRate;
                            if (r.sourceOffset >= r.audio->getNumSamples()) continue;
                            const uint64_t availableSource = r.audio->getNumSamples() - r.sourceOffset;
                            r.sourceLength = r.sourceLength == 0
                                ? std::min(r.len, availableSource)
                                : std::min(r.sourceLength, availableSource);
                            if (state.version < 42 && std::isfinite(decodedSourceRate) &&
                                decodedSourceRate >= 8000.0 && decodedSourceRate <= 384000.0) {
                                // Older projects stored native source-frame
                                // counts as timeline-frame counts. Rust migrates
                                // the duration, trim origin, fades, warp markers
                                // and range windows while preserving the
                                // musical start; an unrepresentable geometry
                                // fails hydration.
                                static_assert(sizeof(Region::WarpMarker) ==
                                              sizeof(HirariLegacyWarpMarker));
                                static_assert(sizeof(Region::RangeEdit) ==
                                              sizeof(HirariLegacyRangeEdit));
                                HirariRegionLegacyMigration migration{};
                                migration.source_offset = r.sourceOffset;
                                migration.base_source_offset = r.baseSourceOffset;
                                migration.source_length = r.sourceLength;
                                migration.base_start = r.baseStart;
                                migration.warp_ratio = r.warpRatio;
                                migration.loop_count = r.loopCount;
                                migration.fade_in = r.fadeInSamples;
                                migration.fade_out = r.fadeOutSamples;
                                migration.decoded_source_rate = decodedSourceRate;
                                migration.project_sample_rate = projectSampleRate;
                                migration.markers = reinterpret_cast<HirariLegacyWarpMarker*>(
                                    r.warpMarkers.data());
                                migration.marker_count = r.warpMarkers.size();
                                migration.range_edits = reinterpret_cast<HirariLegacyRangeEdit*>(
                                    r.rangeEdits.data());
                                migration.range_edit_count = r.rangeEdits.size();
                                if (!hirari_region_legacy_source_rate_migrate(&migration))
                                    return false;
                                r.sourceSampleRate = migration.source_sample_rate;
                                r.start = migration.start;
                                r.len = migration.len;
                                r.fadeInSamples = migration.out_fade_in;
                                r.fadeOutSamples = migration.out_fade_out;
                            }
                        }
                    }
                    // The persisted region length may have been larger than
                    // the decoded media (for example after an external file
                    // was replaced). Rust filters the range edits against the
                    // final playable length so the renderer never evaluates
                    // stale offsets beyond the audio buffer.
                    if (!r.rangeEdits.empty()) {
                        r.rangeEdits.resize(hirari_region_range_edits_sanitize(
                            reinterpret_cast<HirariLegacyRangeEdit*>(r.rangeEdits.data()),
                            r.rangeEdits.size(), r.len));
                    }
                    t->addRegion(r);
                    break;
                }
            }
        }
        // Sidechain plugin indices are checked against the hydrated chain:
        // sandbox admission can drop a stored path, which the document codec
        // cannot predict. Track references, tap points and link cycles were
        // rejected before the state became valid.
        for (const auto& sidechain : state.sidechains) {
            const auto destination = std::find_if(loadedTracks.begin(), loadedTracks.end(),
                [&sidechain](const auto& track) { return track->getId() == sidechain.destinationId; });
            if (destination == loadedTracks.end() ||
                sidechain.pluginIndex >= (*destination)->getPluginTypes().size()) return false;
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
            // Only the uint32 max sentinel is refused here. new_project()
            // never produces ID 0 any more -- the starter track is ID 1 and
            // zero is the invalid sentinel -- but native containers written
            // before that change can still carry a track at 0, so rejecting
            // zero here would fail hydration on reload for files that
            // serialize fine.
            if (!track) return false;
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
        hirari_native_pdc_reset_for_project(m_pdcState);
        sidechainManager.resetForProject();
        hirari_routing_engine_reset(m_routingEngine);
        hirari_bus_system_reset(m_busSystem);
        hirari_control_room_reset(m_controlRoomState);
        m_sidechainRoutes.clear();
        m_regionIdCounter.store(nextRegionId, std::memory_order_release);
        m_trackIdCounter.store(nextTrackId, std::memory_order_release);
        m_tracks = std::move(loadedTracks);
        mark_project_layout_changed();
        for (const auto& route : state.routes) {
            hirari_routing_engine_add_connection(
                m_routingEngine, route.sourceId, route.destinationId, route.gain,
                route.send, route.preFader);
        }
        for (const auto& track : m_tracks) {
            if (auto* busTrack = dynamic_cast<BusTrack*>(track.get())) {
                const uint32_t busId = busTrack->busId();
                busTrack->resolveBus(hirari_bus_system_register(m_busSystem, busId)
                    ? hirari_bus_system_bus(m_busSystem, busId) : nullptr);
            }
        }
        lock.unlock();
        hirari_tempo_map_set_current_bpm(m_tempoState, static_cast<float>(state.bpm));
        m_rootNote.store(state.rootNote, std::memory_order_relaxed);
        m_scaleType.store(state.scaleType, std::memory_order_relaxed);
        // Restore timeline markers as part of the document, not as transient UI state.
        m_arrangement.clearMarkers();
        for (const auto& marker : state.markers) {
            m_arrangement.upsertMarker({marker.sample, std::string(marker.name), marker.color});
        }
        std::vector<Composition::ArrangerPart> arrangerParts;
        arrangerParts.reserve(state.arrangerParts.size());
        for (const auto& part : state.arrangerParts)
            arrangerParts.push_back({part.start, part.length, std::string(part.name), part.repeats});
        m_arrangement.setArrangerParts(std::move(arrangerParts));
        hirari_tempo_map_clear(m_tempoState, projectSampleRate, state.bpm);
        for (const auto& event : state.tempoEvents)
            hirari_tempo_map_add_tempo(m_tempoState, event.sample, event.bpm, projectSampleRate, event.ramp != 0);
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

bool HirariUnifiedEngine::saveProject(std::string_view path) {
    namespace Bridge = ::Hirari::Core::Bridge;
    Bridge::NativeProjectState state;
    state.version = 42;
    state.bpm = hirari_tempo_map_get_current_bpm(m_tempoState);
    state.rootNote = m_rootNote.load(std::memory_order_relaxed);
    state.scaleType = m_scaleType.load(std::memory_order_relaxed);
    state.sampleRate = static_cast<uint32_t>(m_sampleRate.load());

    const auto appendBlob = [](rust::Vec<uint8_t>& destination,
                               const std::vector<uint8_t>& source) {
        destination.reserve(source.size());
        for (uint8_t byte : source) destination.push_back(byte);
    };
    const auto convertAutomation = [](const std::vector<ProjectAutomationPoint>& points) {
        rust::Vec<Bridge::NativeAutomationPoint> converted;
        converted.reserve(points.size());
        for (const auto& point : points) {
            Bridge::NativeAutomationPoint value;
            value.time = point.time;
            value.value = point.value;
            value.curve = point.curve;
            converted.push_back(value);
        }
        return converted;
    };
    const auto convertBlobs = [&appendBlob](const std::vector<std::vector<uint8_t>>& blobs) {
        rust::Vec<Bridge::NativeBlob> converted;
        converted.reserve(blobs.size());
        for (const auto& blob : blobs) {
            Bridge::NativeBlob convertedBlob;
            appendBlob(convertedBlob.bytes, blob);
            converted.push_back(std::move(convertedBlob));
        }
        return converted;
    };

    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for(auto& t : m_tracks) {
        Bridge::NativeTrackState ts;
        ts.id = t->getId();
        ts.trackType = static_cast<uint32_t>(t->getType());
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
        ts.volumeAutomation = convertAutomation(t->getVolumeAutomationPoints());
        ts.panAutomation = convertAutomation(t->getPanAutomationPoints());
        ts.trackDelayAutomation = convertAutomation(t->getTrackDelayAutomationPoints());
        for (const auto& lane : t->getPluginAutomation()) {
            Bridge::NativePluginAutomationLane converted;
            converted.pluginIndex = lane.pluginIndex;
            converted.parameterId = lane.parameterId;
            converted.points = convertAutomation(lane.points);
            ts.pluginAutomation.push_back(std::move(converted));
        }
        ts.pluginName = rust::String(t->getName());
        // Plugin type IDs are positionally meaningful inside a track; store
        // them little-endian so a project file is portable across hosts.
        for (uint32_t pluginType : t->getPluginTypes()) {
            for (uint32_t shift = 0; shift < 32; shift += 8) {
                ts.pluginData.push_back(static_cast<uint8_t>((pluginType >> shift) & 0xffu));
            }
        }
        ts.pluginStates = convertBlobs(t->getPluginStates());
        ts.pluginGuiStates = convertBlobs(t->getPluginGuiStates());
        ts.pluginBypass.reserve(t->getPluginTypes().size());
        for (uint32_t pluginIndex = 0; pluginIndex < t->getPluginTypes().size(); ++pluginIndex) {
            ts.pluginBypass.push_back(t->getPluginBypass(pluginIndex) ? 1 : 0);
        }
        for (const auto& sandboxPath : t->getSandboxedPluginPaths()) {
            ts.sandboxedPluginPaths.push_back(rust::String(sandboxPath));
        }
        ts.sandboxedPluginStates = convertBlobs(t->getSandboxedPluginStates());
        state.tracks.push_back(std::move(ts));
        for (const auto& region : t->getRegions()) {
            Bridge::NativeRegionState rs;
            rs.id = region.id;
            rs.trackId = t->getId();
            rs.samplePosition = region.start;
            rs.sampleLength = region.len;
            rs.sourceLength = region.sourceLength;
            rs.sourceSampleRate = static_cast<uint32_t>(std::clamp(
                std::llround(region.sourceSampleRate > 0.0
                    ? region.sourceSampleRate : state.sampleRate), 8000LL, 384000LL));
            rs.sourceOffset = region.sourceOffset;
            rs.baseStart = region.baseStart;
            rs.baseSourceOffset = region.baseSourceOffset;
            rs.baseLength = region.baseLength;
            rs.isMuted = region.muted;
            rs.filePath = rust::String(region.path);
            rs.name = rust::String(region.name);
            rs.clipGain = region.clipGain;
            rs.fadeInSamples = region.fadeInSamples;
            rs.fadeOutSamples = region.fadeOutSamples;
            rs.reverse = region.reverse;
            rs.warpRatio = region.warpRatio;
            rs.pitchPreserveWarp = region.pitchPreserveWarp;
            rs.pitchSemitones = region.pitchSemitones;
            rs.loopCount = region.loopCount;
            rs.locked = region.locked;
            rs.syncGroup = region.syncGroup;
            rs.warpMarkers.reserve(region.warpMarkers.size());
            for (const auto& marker : region.warpMarkers) {
                Bridge::NativeWarpMarker converted;
                converted.sourceSample = marker.sourceSample;
                converted.timelineSample = marker.timelineSample;
                converted.transient = marker.transient;
                rs.warpMarkers.push_back(converted);
            }
            std::vector<::hirari::editing::EventProcessingStep> history;
            if (!region.processingHistory.copy(history)) return {};
            for (const auto& step : history) {
                Bridge::NativeProcessingStep converted;
                converted.id = step.id;
                converted.operation = rust::String(step.operation);
                converted.parameter = step.parameter;
                converted.enabled = step.enabled;
                rs.processingHistory.push_back(std::move(converted));
            }
            std::vector<::hirari::editing::AudioNoteSegment> noteSegments;
            if (!region.noteEdits.copy(noteSegments)) return {};
            for (const auto& segment : noteSegments) {
                Bridge::NativeNoteSegment converted;
                converted.startSeconds = segment.startSeconds;
                converted.endSeconds = segment.endSeconds;
                converted.detectedPitchCents = segment.detectedPitchCents;
                converted.pitchOffsetCents = segment.pitchOffsetCents;
                converted.formantOffsetCents = segment.formantOffsetCents;
                converted.anchors.reserve(segment.anchors.size());
                for (const auto& anchor : segment.anchors) {
                    Bridge::NativeNoteAnchor value;
                    value.positionSeconds = anchor.positionSeconds;
                    value.pitchCents = anchor.pitchCents;
                    value.formantCents = anchor.formantCents;
                    converted.anchors.push_back(value);
                }
                rs.audioNoteSegments.push_back(std::move(converted));
            }
            for (const auto& edit : region.rangeEdits) {
                Bridge::NativeRangeEdit converted;
                converted.start = edit.start;
                converted.end = edit.end;
                converted.gain = edit.gain;
                converted.fadeIn = edit.fadeIn;
                converted.fadeOut = edit.fadeOut;
                rs.rangeEdits.push_back(converted);
            }
            state.regions.push_back(std::move(rs));
        }
    }
    for (const auto& sidechain : m_sidechainRoutes) {
        Bridge::NativeSidechainState serialized;
        serialized.sourceId = sidechain.sourceId;
        serialized.destinationId = sidechain.destinationId;
        serialized.pluginIndex = sidechain.pluginIndex;
        // Route identity is persisted independently from native buffer
        // pointers; those are rebound after the project is loaded.
        serialized.tapPoint = sidechain.tapPoint;
        state.sidechains.push_back(serialized);
    }
    for (const auto& marker : m_arrangement.markers()) {
        Bridge::NativeMarkerState serialized;
        serialized.sample = marker.sample;
        serialized.name = rust::String(marker.name);
        serialized.color = marker.color;
        state.markers.push_back(std::move(serialized));
    }
    for (const auto& part : m_arrangement.arrangerParts()) {
        Bridge::NativeArrangerPartState serialized;
        serialized.start = part.start; serialized.length = part.length;
        serialized.name = rust::String(part.name); serialized.repeats = part.repeats;
        state.arrangerParts.push_back(std::move(serialized));
    }
    for (const auto& event : readTempoEvents(m_tempoState)) {
        Bridge::NativeTempoEventState serialized;
        serialized.sample = event.samplePos; serialized.bpm = event.bpm; serialized.ramp = event.ramp ? 1 : 0;
        state.tempoEvents.push_back(serialized);
    }
    size_t routeCount = 0;
    void* rawRoutes = hirari_routing_engine_copy_connections(m_routingEngine, &routeCount);
    std::vector<Hirari::Core::Engine::RoutingEngine::Connection> routeConnections =
        takeRoutingConnections(rawRoutes, routeCount);
    state.routes.reserve(routeConnections.size());
    for (const auto& connection : routeConnections) {
        Bridge::NativeRouteState serialized;
        serialized.sourceId = connection.sourceId;
        serialized.destinationId = connection.destId;
        serialized.gain = connection.gain;
        serialized.send = connection.send;
        serialized.preFader = connection.preFader;
        state.routes.push_back(serialized);
    }

    return Bridge::native_project_save(rust::Str(path.data(), path.size()), state);
}

rust::Vec<uint8_t> HirariUnifiedEngine::get_video_frame() {
    rust::Vec<uint8_t> frame;
#if defined(__APPLE__)
    std::vector<uint8_t> rgba;
    uint32_t width = 0, height = 0;
    if (::Hirari::Core::Engine::VideoBridge::copyCurrentFrame(rgba, width, height) &&
        width > 0 && height > 0 && rgba.size() == static_cast<size_t>(width) * height * 4u) {
        // Packet: little-endian width, height, followed by packed RGBA8.
        frame.reserve(8u + rgba.size());
        for (uint32_t shift = 0; shift < 32; shift += 8) frame.push_back(static_cast<uint8_t>((width >> shift) & 0xffu));
        for (uint32_t shift = 0; shift < 32; shift += 8) frame.push_back(static_cast<uint8_t>((height >> shift) & 0xffu));
        for (uint8_t value : rgba) frame.push_back(value);
    }
#endif
    // Without the platform video bridge there is no frame producer: the
    // Rust video runtime owns decoding and caching, so this stays empty.
    return frame;
}
