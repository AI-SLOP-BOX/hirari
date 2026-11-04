            loadedTracks.push_back(t);
        }

        // Rust rebuilds each chain and curve set from the decoded document now
        // that every provisional track has its stores. An insert the engine
        // cannot create fails hydration, leaving the previous project published.
        if (!::Hirari::Core::Bridge::native_project_hydrate_plugins(state, pluginTargets)) {
            return false;
        }
        
        // 2. Hydrate Regions.
        //
        // The walk is Rust-owned (region_hydration.rs): media decoding, the
        // pre-v28 id handout, the clamp/migrate policy and the persisted
        // warp, range-edit, history and note tables all live there. The engine
        // only names the provisional tracks the regions install into.
        rust::Vec<::Hirari::Core::Bridge::NativeTrackHandle> regionTracks;
        regionTracks.reserve(loadedTracks.size());
        for (const auto& track : loadedTracks) {
            regionTracks.push_back(
                ::Hirari::Core::Bridge::NativeTrackHandle{track->getId(),
                    reinterpret_cast<size_t>(track.get())});
        }
        if (!::Hirari::Core::Bridge::native_project_hydrate_regions(
                state, regionTracks, projectSampleRate)) {
            return false;
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
        // the old project remained published.  The ordered teardown of the
        // outgoing graph and of every project-scoped manager is Rust's decision
        // (engine_track_table.rs); only the replacement graph is inserted here.
        hirari_engine_reset_project(m_trackTable, m_engineRings, m_pdcState,
                                    m_sidechainEngine, m_routingEngine, m_busSystem,
                                    m_controlRoomState, m_sidechainRoutes);
        for (auto& track : loadedTracks) {
            Track* raw = track.get();
            (void)hirari_engine_track_insert(m_trackTable, std::numeric_limits<size_t>::max(),
                                   newTrackHandle(std::move(track)), raw);
        }
        loadedTracks.clear();
        mark_project_layout_changed();
        for (const auto& route : state.routes) {
            hirari_routing_engine_add_connection(
                m_routingEngine, route.sourceId, route.destinationId, route.gain,
                route.send, route.preFader);
        }
        for (Track* track : lockedTracks()) {
            if (auto* busTrack = dynamic_cast<BusTrack*>(track)) {
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

