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
        for (auto& rs : state.regions) {
            for (auto& t : loadedTracks) {
                if (t->getId() != rs.trackId) continue;
                // Versions before 28 stored no region ids, so hydration hands
                // them out in document order. The allocator resumes past the
                // highest id once the document is installed.
                const uint32_t regionId = state.version >= 28 ? rs.id : nextRegionId++;
                // Decode before hydration so Rust can clamp the region to the
                // media that actually exists and migrate legacy geometry.
                std::shared_ptr<AudioBuffer> audio;
                double decodedSourceRate = 0.0;
                std::error_code fileError;
                const std::string regionPath(rs.filePath);
                const std::filesystem::path resolvedPath(regionPath);
                if (std::filesystem::is_regular_file(resolvedPath, fileError) && !fileError) {
                    audio = importAudioFile(regionPath, &decodedSourceRate);
                    if (audio && (audio->getNumSamples() == 0 || audio->getNumChannels() == 0)) {
                        audio.reset();
                    }
                }
                HirariRegionState hydrated{};
                const uint8_t hydration = hirari_region_hydrate_decoded(
                    &rs, audio ? audio->getNumSamples() : 0u, decodedSourceRate,
                    projectSampleRate, state.version, &hydrated);
                if (hydration == 1) continue;
                if (hydration == 2) return false;
                // Saved tables are installed verbatim; adding the region
                // re-normalizes it inside the Rust store.
                auto* draft = hirari_region_store_draft_new(
                    regionId, reinterpret_cast<const uint8_t*>(regionPath.data()),
                    regionPath.size(), reinterpret_cast<const uint8_t*>(rs.name.data()),
                    rs.name.size(), &hydrated);
                std::unique_ptr<HirariRegionDraft, void (*)(HirariRegionDraft*)> draftGuard{
                    draft, &hirari_region_store_draft_discard};
                static_assert(sizeof(rs.warpMarkers[0]) == sizeof(HirariWarpMarker));
                static_assert(sizeof(rs.rangeEdits[0]) == sizeof(HirariRegionRangeEdit));
                auto* clip = hirari_region_store_draft_clip(draft);
                (void)hirari_region_clip_edits_set_warp_markers(
                    clip, reinterpret_cast<const HirariWarpMarker*>(rs.warpMarkers.data()),
                    rs.warpMarkers.size(), 0, 0, false);
                (void)hirari_region_clip_edits_replace_range_edits(
                    clip, reinterpret_cast<const HirariRegionRangeEdit*>(rs.rangeEdits.data()),
                    rs.rangeEdits.size(), hydrated.len, false);
                auto* history = hirari_region_store_draft_history(draft);
                for (const auto& step : rs.processingHistory) {
                    const HirariEventStep entry{
                        step.id, reinterpret_cast<const uint8_t*>(step.operation.data()),
                        step.operation.size(), step.parameter, step.enabled};
                    if (!hirari_region_processing_history_append(history, entry)) return false;
                }
                if (!hirari_region_note_store_load_native(
                        reinterpret_cast<size_t>(hirari_region_store_draft_notes(draft)),
                        rs.audioNoteSegments)) {
                    return false;
                }
                // Hydrate audio during project load so an opened project is
                // immediately playable. Missing files remain visible as
                // unloaded regions and can be repaired via Relink.
                if (!hirari_region_store_draft_add(
                        draftGuard.release(), t.get(),
                        audio ? new std::shared_ptr<AudioBuffer>(std::move(audio)) : nullptr)) {
                    return false;
                }
                break;
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
        
        // Commit point: all validation and hydration above completed while
        // the old project remained published.  Only now replace the graph,
        // reset project-scoped managers, and publish fresh ID allocators.
        for (const auto& track : m_tracks) {
            hirari_sidechain_engine_remove_track(m_sidechainEngine, track->getId());
        }
        hirari_engine_command_ring_clear(m_engineRings);
        hirari_native_pdc_reset_for_project(m_pdcState);
        hirari_sidechain_engine_reset(m_sidechainEngine);
        hirari_routing_engine_reset(m_routingEngine);
        hirari_bus_system_reset(m_busSystem);
        hirari_control_room_reset(m_controlRoomState);
        hirari_sidechain_routes_clear(m_sidechainRoutes);
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
        hirari_ids_seed_from_project(this);
        hirari_tempo_map_set_current_bpm(m_tempoState, static_cast<float>(state.bpm));
        m_rootNote.store(state.rootNote, std::memory_order_relaxed);
        m_scaleType.store(state.scaleType, std::memory_order_relaxed);
        // Restore timeline markers as part of the document, not as transient UI state.
        hirari_arrangement_document_reset_markers(m_arrangementDocument);
        for (const auto& marker : state.markers) {
            (void)hirari_arrangement_document_upsert_marker(
                m_arrangementDocument, marker.sample,
                reinterpret_cast<const uint8_t*>(marker.name.data()), marker.name.size(),
                marker.color);
        }
        std::vector<HirariArrangementPartInput> arrangerParts;
        arrangerParts.reserve(state.arrangerParts.size());
        for (const auto& part : state.arrangerParts) {
            arrangerParts.push_back({part.start, part.length,
                reinterpret_cast<const uint8_t*>(part.name.data()), part.name.size(),
                part.repeats});
        }
        (void)hirari_arrangement_document_set_parts(
            m_arrangementDocument, arrangerParts.data(), arrangerParts.size());
        hirari_tempo_map_clear(m_tempoState, projectSampleRate, state.bpm);
        for (const auto& event : state.tempoEvents)
            hirari_tempo_map_add_tempo(m_tempoState, event.sample, event.bpm, projectSampleRate, event.ramp != 0);
        // A project carries its own timeline rate. Re-prepare every track and
        // DSP component so opening a 48/96 kHz project does not leave the
        // engine running at the previous device rate.
        prepareToPlay(projectSampleRate, currentBlockSize);
        for (const auto& sidechain : state.sidechains) {
            if (!hirari_routing_edit_set_sidechain_link(
                    this, sidechain.sourceId, sidechain.destinationId, sidechain.pluginIndex,
                    sidechain.tapPoint, true)) {
                return false;
            }
        }
        // A loaded project starts a fresh editing history. Keeping commands
        // from the previous document would let Undo mutate the wrong graph.
        hirari_undo_manager_clear(m_undoState);
        return true;
    }
    return false;
}

bool HirariUnifiedEngine::saveProject(std::string_view path) {
    // Rust gathers the same model as the layout JSON writer and encodes the
    // container itself; field mapping, bounds and plug-in id encoding live in
    // hirari-core-bridge/src/project_save_layout.rs.
    return hirari_project_save_from_host(
        this, layoutHostOps(), reinterpret_cast<const uint8_t*>(path.data()), path.size(),
        m_tempoState, m_routingEngine, reinterpret_cast<size_t>(m_arrangementDocument),
        hirari_tempo_map_get_current_bpm(m_tempoState),
        m_rootNote.load(std::memory_order_relaxed),
        m_scaleType.load(std::memory_order_relaxed));
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
