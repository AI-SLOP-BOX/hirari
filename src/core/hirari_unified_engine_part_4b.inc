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
                    r.sourceLength = rs.sourceLength;
                    r.sourceSampleRate = rs.sourceSampleRate > 0
                        ? rs.sourceSampleRate : projectSampleRate;
                    r.timelineSampleRate = projectSampleRate;
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
                                // counts as timeline-frame counts. Migrate the
                                // duration and trim origin using the decoded
                                // file rate while preserving the musical start.
                                r.sourceSampleRate = decodedSourceRate;
                                const double sourcePerTimeline = r.warpRatio *
                                    decodedSourceRate / projectSampleRate;
                                if (!std::isfinite(sourcePerTimeline) || sourcePerTimeline <= 0.0)
                                    return false;
                                const uint64_t sourceTrim = r.sourceOffset >= r.baseSourceOffset
                                    ? r.sourceOffset - r.baseSourceOffset : 0;
                                const long double migratedTrim = std::ceil(
                                    static_cast<long double>(sourceTrim) / sourcePerTimeline);
                                const long double migratedLength = std::ceil(
                                    static_cast<long double>(r.sourceLength) / sourcePerTimeline);
                                if (!std::isfinite(static_cast<double>(migratedTrim)) ||
                                    !std::isfinite(static_cast<double>(migratedLength)) ||
                                    migratedTrim > static_cast<long double>(UINT64_MAX) ||
                                    migratedLength < 1.0L ||
                                    migratedLength > static_cast<long double>(UINT64_MAX) ||
                                    r.baseStart > UINT64_MAX - static_cast<uint64_t>(migratedTrim))
                                    return false;
                                r.start = r.baseStart + static_cast<uint64_t>(migratedTrim);
                                r.len = static_cast<uint64_t>(migratedLength);
                                if (r.start > UINT64_MAX - r.len || r.len > UINT64_MAX / r.loopCount ||
                                    r.start > UINT64_MAX - r.len * r.loopCount) return false;
                                const double timelineScale = projectSampleRate / decodedSourceRate;
                                const auto migrateTimelineSamples = [timelineScale](uint64_t value) {
                                    const long double scaled = std::round(
                                        static_cast<long double>(value) * timelineScale);
                                    return static_cast<uint64_t>(std::clamp(
                                        scaled, 0.0L, static_cast<long double>(UINT64_MAX)));
                                };
                                r.fadeInSamples = std::min(r.len,
                                    migrateTimelineSamples(r.fadeInSamples));
                                r.fadeOutSamples = std::min(r.len,
                                    migrateTimelineSamples(r.fadeOutSamples));
                                for (auto& marker : r.warpMarkers)
                                    marker.timelineSample = migrateTimelineSamples(marker.timelineSample);
                                for (auto& edit : r.rangeEdits) {
                                    edit.start = migrateTimelineSamples(edit.start);
                                    edit.end = std::min(r.len, migrateTimelineSamples(edit.end));
                                    edit.fadeIn = migrateTimelineSamples(edit.fadeIn);
                                    edit.fadeOut = migrateTimelineSamples(edit.fadeOut);
                                }
                            }
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
        m_busSystem.resetForProject();
        m_controlRoom.resetForProject();
        m_sidechainRoutes.clear();
        m_regionIdCounter.store(nextRegionId, std::memory_order_release);
        m_trackIdCounter.store(nextTrackId, std::memory_order_release);
        m_tracks = std::move(loadedTracks);
        mark_project_layout_changed();
        for (const auto& route : state.routes) {
            m_routingEngine.addConnection(route.sourceId, route.destinationId, route.gain,
                                          route.send, route.preFader);
        }
        for (const auto& track : m_tracks) {
            if (auto* busTrack = dynamic_cast<BusTrack*>(track.get())) {
                busTrack->resolveBus(m_busSystem.registerBus(busTrack->busId()));
            }
        }
        lock.unlock();
        m_tempoMap.setCurrentBpm(static_cast<float>(state.bpm));
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

bool HirariUnifiedEngine::saveProject(std::string_view path) {
    ProjectSerializer::ProjectState state;
    state.bpm = m_tempoMap.currentBpm();
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
        ts.pluginAutomation = t->getPluginAutomation();
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
            rs.sourceLength = region.sourceLength;
            rs.sourceSampleRate = static_cast<uint32_t>(std::clamp(
                std::llround(region.sourceSampleRate > 0.0
                    ? region.sourceSampleRate : state.sampleRate), 8000LL, 384000LL));
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
            rs.pitchPreserveWarp = region.pitchPreserveWarp;
            rs.pitchSemitones = region.pitchSemitones;
            rs.loopCount = region.loopCount;
            rs.locked = region.locked;
            rs.syncGroup = region.syncGroup;
            rs.warpMarkers.clear();
            rs.warpMarkers.reserve(region.warpMarkers.size());
            for (const auto& marker : region.warpMarkers) {
                rs.warpMarkers.push_back({marker.sourceSample, marker.timelineSample,
                                          marker.transient});
            }
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
    std::vector<Hirari::Core::Engine::RoutingEngine::Connection> routeConnections;
    m_routingEngine.copyConnections(routeConnections);
    state.routes.reserve(routeConnections.size());
    for (const auto& connection : routeConnections) {
        ProjectSerializer::RouteState serialized;
        serialized.sourceId = connection.sourceId;
        serialized.destinationId = connection.destId;
        serialized.gain = connection.gain;
        serialized.send = connection.send;
        serialized.preFader = connection.preFader;
        state.routes.push_back(serialized);
    }
    
    // Note: SBF-v6 enables CRC32 and Endian-Safety automatically in saveAtomic
    return ProjectSerializer::saveAtomic(std::string(path), state);
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
        return frame;
    }
#endif
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    if (!std::isfinite(sampleRate) || sampleRate <= 0.0) return frame;
#if !defined(__APPLE__)
    ::Hirari::Core::Scheduler::VideoFrame source;
    const double seconds = static_cast<double>(m_playhead.load(std::memory_order_relaxed)) / sampleRate;
    if (!::Hirari::Core::Scheduler::VideoEngine::getInstance().copyFrameAt(seconds, source) ||
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
