            if (state.trackCount < kMaxTracks) state.activeTracks[state.trackCount++] = track.get();
        }
        m_activeStateIdx.store(nextIdx, std::memory_order_release);
    }

    const auto removeCopy = [this, newId]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
                               [newId](const auto& track) { return track->getId() == newId; });
        if (it == m_tracks.end()) return;
        for (const auto& region : (*it)->getRegions()) {
            hirari_region_waveform_forget(this, region.id);
        }
        m_deferredDestructionTracks.push_back(*it);
        m_tracks.erase(it);
        mark_project_layout_changed();
        uint32_t nextIdx = 1 - m_activeStateIdx.load(std::memory_order_acquire);
        auto& state = m_audioStates[nextIdx];
        state.trackCount = 0;
        for (const auto& track : m_tracks) {
            if (state.trackCount < kMaxTracks) state.activeTracks[state.trackCount++] = track.get();
        }
        m_activeStateIdx.store(nextIdx, std::memory_order_release);
    };
    const auto restoreCopy = [this, copy, newId, insertIndex]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        const bool alreadyPresent = std::any_of(m_tracks.begin(), m_tracks.end(),
            [newId](const auto& track) { return track->getId() == newId; });
        if (alreadyPresent) return;
        m_tracks.insert(m_tracks.begin() + std::min(insertIndex, m_tracks.size()), copy);
        mark_project_layout_changed();
        uint32_t nextIdx = 1 - m_activeStateIdx.load(std::memory_order_acquire);
        auto& state = m_audioStates[nextIdx];
        state.trackCount = 0;
        for (const auto& track : m_tracks) {
            if (state.trackCount < kMaxTracks) state.activeTracks[state.trackCount++] = track.get();
        }
        m_activeStateIdx.store(nextIdx, std::memory_order_release);
    };
    m_undoManager.recordAppliedAction("Duplicate Track", removeCopy, restoreCopy);
    syncStructuralChanges();
    return newId;
}

bool HirariUnifiedEngine::remove_track(uint32_t tid) {
    const auto sidechainBackup = m_sidechainManager.captureLinksForTrack(tid);
    std::shared_ptr<Track> removed;
    std::vector<SidechainRoute> sidechainRouteBackup;
    std::vector<Hirari::Core::Engine::RoutingEngine::Connection> routingBackup;
    std::vector<Hirari::Core::Engine::RoutingEngine::FeedbackConnection> feedbackBackup;
    size_t originalIndex = 0;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
                               [tid](const auto& track) { return track->getId() == tid; });
        if (it == m_tracks.end()) return false;
        originalIndex = static_cast<size_t>(it - m_tracks.begin());
        removed = *it;
        m_sidechainManager.removeLinksForTrack(tid);
        for (const auto& route : m_sidechainRoutes) {
            if (route.sourceId == tid || route.destinationId == tid)
                sidechainRouteBackup.push_back(route);
        }
        m_sidechainRoutes.erase(
            std::remove_if(m_sidechainRoutes.begin(), m_sidechainRoutes.end(),
                [tid](const SidechainRoute& route) {
                    return route.sourceId == tid || route.destinationId == tid;
                }),
            m_sidechainRoutes.end());
        m_routingEngine.removeProcessingDependenciesForNode(tid);
        m_routingEngine.removeConnectionsForNode(tid, routingBackup);
        m_routingEngine.removeFeedbackConnectionsForNode(tid, feedbackBackup);
        {
            for (const auto& region : removed->getRegions()) {
                hirari_region_waveform_forget(this, region.id);
            }
        }
        m_deferredDestructionTracks.push_back(removed);
        m_tracks.erase(it);
        mark_project_layout_changed();
        if (tid == 1 && m_tracks.empty()) {
            // new_project() creates the bootstrap at ID 1 and advances the
            // allocator to 2. Removing that placeholder during project
            // hydration must rewind the allocator so the first persisted track
            // keeps its canonical ID instead of becoming ID 2.
            m_trackIdCounter.store(1, std::memory_order_release);
        }
        uint32_t nextIdx = 1 - m_activeStateIdx.load(std::memory_order_acquire);
        auto& state = m_audioStates[nextIdx];
        state.trackCount = 0;
        for (const auto& track : m_tracks) {
            if (state.trackCount < kMaxTracks) state.activeTracks[state.trackCount++] = track.get();
        }
        m_activeStateIdx.store(nextIdx, std::memory_order_release);
    }

    const auto removeAgain = [this, tid]() {
        {
            std::lock_guard<std::mutex> lock(m_tracksMutex);
            m_sidechainManager.removeLinksForTrack(tid);
            m_sidechainRoutes.erase(
                std::remove_if(m_sidechainRoutes.begin(), m_sidechainRoutes.end(),
                    [tid](const SidechainRoute& route) {
                        return route.sourceId == tid || route.destinationId == tid;
                    }),
                m_sidechainRoutes.end());
            m_routingEngine.removeProcessingDependenciesForNode(tid);
            auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
                                   [tid](const auto& track) { return track->getId() == tid; });
            if (it == m_tracks.end()) return;
            for (const auto& region : (*it)->getRegions()) {
                hirari_region_waveform_forget(this, region.id);
            }
            m_deferredDestructionTracks.push_back(*it);
            m_tracks.erase(it);
            mark_project_layout_changed();
            uint32_t nextIdx = 1 - m_activeStateIdx.load(std::memory_order_acquire);
            auto& state = m_audioStates[nextIdx];
            state.trackCount = 0;
            for (const auto& track : m_tracks) {
                if (state.trackCount < kMaxTracks) state.activeTracks[state.trackCount++] = track.get();
            }
            m_activeStateIdx.store(nextIdx, std::memory_order_release);
        }
        std::vector<Hirari::Core::Engine::RoutingEngine::Connection> discardedRoutes;
        m_routingEngine.removeConnectionsForNode(tid, discardedRoutes);
        std::vector<Hirari::Core::Engine::RoutingEngine::FeedbackConnection> discardedFeedback;
        m_routingEngine.removeFeedbackConnectionsForNode(tid, discardedFeedback);
    };
    const auto restore = [this, removed, tid, originalIndex, sidechainBackup,
                          sidechainRouteBackup,
                          routingBackup, feedbackBackup]() {
        {
            std::lock_guard<std::mutex> lock(m_tracksMutex);
            const bool alreadyPresent = std::any_of(m_tracks.begin(), m_tracks.end(),
                [tid](const auto& track) { return track->getId() == tid; });
            if (!alreadyPresent) {
                m_tracks.insert(m_tracks.begin() + std::min(originalIndex, m_tracks.size()), removed);
                mark_project_layout_changed();
                uint32_t nextIdx = 1 - m_activeStateIdx.load(std::memory_order_acquire);
                auto& state = m_audioStates[nextIdx];
                state.trackCount = 0;
                for (const auto& track : m_tracks) {
                    if (state.trackCount < kMaxTracks) state.activeTracks[state.trackCount++] = track.get();
                }
                m_activeStateIdx.store(nextIdx, std::memory_order_release);
            }
            for (const auto& route : sidechainRouteBackup) {
                const auto duplicate = std::find_if(
                    m_sidechainRoutes.begin(), m_sidechainRoutes.end(),
                    [&route](const SidechainRoute& current) {
                        return current.destinationId == route.destinationId &&
                               current.pluginIndex == route.pluginIndex;
                    });
                if (duplicate == m_sidechainRoutes.end()) {
                    if (m_routingEngine.addProcessingDependency(
                            route.sourceId, route.destinationId)) {
                        m_sidechainRoutes.push_back(route);
                    }
                }
            }
        }
        m_sidechainManager.restoreLinks(sidechainBackup);
        for (const auto& connection : routingBackup)
            m_routingEngine.addConnection(connection.sourceId, connection.destId, connection.gain,
                                          connection.send, connection.preFader);
        for (const auto& connection : feedbackBackup)
            m_routingEngine.addFeedbackConnection(connection.sourceId, connection.destId, connection.gain);
    };
    m_undoManager.recordAppliedAction("Remove Track", restore, removeAgain);
    syncStructuralChanges();
    return true;
}

bool HirariUnifiedEngine::set_track_name(uint32_t tid, std::string_view name) {
    return hirari_control_edit_rename_track(
        this, tid, reinterpret_cast<const uint8_t*>(name.data()), name.size());
}

float HirariUnifiedEngine::get_track_volume(uint32_t tid) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid) return track->getVolume();
    }
    return -1.0f;
}

bool HirariUnifiedEngine::set_track_solo_for_offline_render(uint32_t tid, bool solo) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track && track->getId() == tid) {
            // Offline stem selection is render transaction state, not an
            // editable user action. Do not append it to Undo/Redo.
            track->setSolo(solo);
            return true;
        }
    }
    return false;
}

bool HirariUnifiedEngine::set_offline_render_target(uint32_t tid) {
    // 0 is reserved for the master target. Track/bus IDs are validated
    // against the current graph before activation so a stale CLI target
    // cannot silently render an unrelated output.
    if (tid != 0) {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        const bool exists = std::any_of(m_tracks.begin(), m_tracks.end(),
            [tid](const auto& track) { return track && track->getId() == tid; });
        if (!exists) return false;
    }
    m_offlineTargetId.store(tid, std::memory_order_release);
    m_offlineTargetActive.store(true, std::memory_order_release);
    return true;
}

void HirariUnifiedEngine::clear_offline_render_target() noexcept {
    m_offlineTargetActive.store(false, std::memory_order_release);
    m_offlineTargetId.store(0, std::memory_order_release);
    m_offlineRenderTailMillis.store(0, std::memory_order_release);
    m_offlineRenderPreFader.store(false, std::memory_order_release);
    m_offlineRenderIncludeInserts.store(true, std::memory_order_release);
}

void HirariUnifiedEngine::set_offline_render_tail_seconds(float seconds) noexcept {
    if (!std::isfinite(seconds) || seconds < 0.0f) {
        m_offlineRenderTailMillis.store(0, std::memory_order_release);
        return;
    }
    const float bounded = std::min(seconds, 60.0f);
    m_offlineRenderTailMillis.store(
        static_cast<uint32_t>(bounded * 1000.0f), std::memory_order_release);
}

void HirariUnifiedEngine::set_offline_render_options(bool preFader, bool includeInserts) noexcept {
    m_offlineRenderPreFader.store(preFader, std::memory_order_release);
    m_offlineRenderIncludeInserts.store(includeInserts, std::memory_order_release);
}

void HirariUnifiedEngine::prepareToPlay(double sr, uint32_t bs) {
    // A failed device negotiation must not advance the audio configuration
    // generation or partially replace the currently valid preparation state.
    if (!std::isfinite(sr) || sr < 8'000.0 || sr > 384'000.0 || bs == 0 ||
        bs > 16'384u) {
        return;
    }
    // Close callback admission before waiting for in-flight callbacks. The
    // generation check alone cannot make master/track buffer reallocation
    // safe while a callback still traverses the previous buffers.
    m_audioConfigTransition.store(true, std::memory_order_release);
    struct ConfigTransitionGuard {
        std::atomic<bool>& flag;
        ~ConfigTransitionGuard() { flag.store(false, std::memory_order_release); }
    } transitionGuard{m_audioConfigTransition};
    waitForAudioCallbacks();
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);

        // Deferred Track objects are intentionally retained until shutdown.
        // prepareToPlay may race with a device callback that still has the
        // previous AudioThreadState, so transport state is not a reclamation
        // proof.
        m_sampleRate.store(sr);
        m_audioConfigGeneration.fetch_add(1, std::memory_order_acq_rel);
        m_masterBuffer.setSize(2, bs);
        m_blockMidi.clear();
        m_context->sampleRate = sr;
        m_context->blockSize = bs;
        m_context->audioConfigGeneration = m_audioConfigGeneration.load(std::memory_order_acquire);

        m_masterLimiter->prepareToPlay(sr, bs);
        if (m_analyzer) m_analyzer->prepareToPlay(sr, bs);

        for (auto& t : m_tracks) {
            // A frozen buffer is sample-rate dependent. Never continue using
            // a 44.1 kHz snapshot after a 48/96 kHz device transition; that
            // would silently change timing and invalidate the persisted
            // render identity. The UI can issue a new freeze explicitly.
            if (t->isFrozen() && t->frozenSampleRate() != static_cast<uint32_t>(sr)) {
                t->clearFrozenAudio();
                t->collectRetiredFrozenAudio();
            }
            t->prepareToPlay(sr, bs);
            if (t->canProcess(bs)) {
                auto& work = t->getWorkBuffer(bs);
                m_sidechainManager.controlRefreshSourceBuffer(
                    t->getId(), work.getReadPointer(0),
                    work.getNumChannels() > 1 ? work.getReadPointer(1) : work.getReadPointer(0),
                    bs, static_cast<uint32_t>(sr),
                    m_audioConfigGeneration.load(std::memory_order_acquire));
            }
        }
    }
    // syncStructuralChanges() acquires m_tracksMutex itself. Never call it
    // while the preparation lock is held.
    syncStructuralChanges();
}

void HirariUnifiedEngine::syncStructuralChanges() {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    std::array<bool, Hirari::Core::Engine::BusSystem::kMaxBuses> activeBuses{};
    for (const auto& track : m_tracks) {
        auto* busTrack = track && track->getType() == Track::Type::Bus
            ? dynamic_cast<BusTrack*>(track.get()) : nullptr;
        if (busTrack && busTrack->busId() < activeBuses.size()) {
            activeBuses[busTrack->busId()] = true;
        }
    }
    m_busSystem.reconcileBuses(activeBuses);
    auto& routing = m_routingEngine;
    routing.buildGraph();
    
    // Publish the audio-thread track snapshot.
    uint32_t nextIdx = 1 - m_activeStateIdx.load();
    auto& state = m_audioStates[nextIdx];
    state.audioConfigGeneration = m_audioConfigGeneration.load(std::memory_order_acquire);
    state.trackCount = 0;
    // INDUSTRIAL: Reset the O(1) track map.
    std::memset(state.trackMap, 0, sizeof(state.trackMap));
    
    for (const auto& t : m_tracks) {
        if (state.trackCount < 256) {
            state.activeTracks[state.trackCount++] = t.get();
            // Map track pointer by its ID for O(1) resolution.
            if (t->getId() < 2048) {
                state.trackMap[t->getId()] = t.get();
            }
        }
    }
    
    m_activeStateIdx.store(nextIdx, std::memory_order_release);
    m_graphDirty.store(false);

    // Keep PDC on the same published routing graph as audio processing. The
    // solver runs here on the control thread; Track applies the resulting
    // bounded offset with a preallocated delay line at block boundaries.
    const void* pdc = m_pdcState;
    // This is a full graph rebuild. Clear stale entries for deleted tracks
    // and buses before applying the current snapshot, while retaining the
    // previous active offsets until recalculate() publishes the new graph.
    hirari_native_pdc_clear_configuration(pdc);
    for (const auto& track : m_tracks) {
        if (!track || track->getId() >= kPdcMaxTracks) continue;
        hirari_native_pdc_set_track_destination(pdc, track->getId(), kPdcMasterDestination);
        hirari_native_pdc_set_track_latency(pdc, track->getId(), track->getTotalLatencySamples());
    }
    std::vector<Hirari::Core::Engine::RoutingEngine::Connection> connections;
    routing.copyConnections(connections);
    for (const auto& connection : connections) {
        if (connection.send) {
            if (connection.sourceId < kPdcMaxTracks &&
                connection.destId < kPdcMaxTracks) {
                hirari_native_pdc_set_send_route(pdc, connection.sourceId, connection.destId, true);
            }
            continue;
        }
        if (connection.sourceId < kPdcMaxTracks &&
            connection.destId < kPdcMaxTracks) {
            hirari_native_pdc_set_track_destination(pdc, connection.sourceId, connection.destId);
        }
    }
    hirari_native_pdc_recalculate(pdc);
    for (const auto& connection : connections) {
        if (!connection.send ||
            connection.sourceId >= Hirari::Core::Engine::RoutingEngine::kMaxNodes ||
            connection.destId >= Hirari::Core::Engine::RoutingEngine::kMaxNodes) continue;
        routing.setSendPdcCompensationSamples(
            connection.sourceId, connection.destId,
            hirari_native_pdc_get_send_offset(pdc, connection.sourceId, connection.destId));
    }
    for (const auto& track : m_tracks) {
        if (track && track->getId() < kPdcMaxTracks) {
            track->setPdcCompensationSamples(
                hirari_native_pdc_get_track_offset(pdc, track->getId()));
        }
    }

    // --- BLACK BOX SYNC ---
    auto& bb = ::Hirari::Core::Diagnostics::LogBuffer::BlackBoxRegister::getInstance();
    bb.lastTrackCount.store((uint32_t)m_tracks.size(), std::memory_order_relaxed);
    bb.lastSyncTimestamp.store(::Hirari::Core::Diagnostics::LogBuffer::getTimestamp(), std::memory_order_relaxed);
    
    ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0x1, "Structural Sync: Topology Rebuilt.");
}

// Region import policy (validation, decoder dispatch, ticket limits, worker
// thread, undo capture) lives in Rust (region_jobs.rs). These wrappers only
// translate the result codes into the existing UI diagnostics.
bool HirariUnifiedEngine::add_region(uint32_t tid, std::string_view path, double start_smp) {
    const int32_t result = hirari_region_import_add(
        this, tid, reinterpret_cast<const uint8_t*>(path.data()), path.size(), start_smp);
    if (result == 1) {
        ::Hirari::Core::Diagnostics::LogBuffer::post(1, 0x2102, "REGION_TRACK_NOT_FOUND");
    } else if (result == 2) {
        ::Hirari::Core::Diagnostics::LogBuffer::post(1, 0x2101, "REGION_IMPORT_FAILED");
    }
    return result == 0;
}

uint64_t HirariUnifiedEngine::queue_region_import(
    uint32_t tid, std::string_view path, uint64_t startSample) {
    return hirari_region_import_queue(
        this, tid, reinterpret_cast<const uint8_t*>(path.data()), path.size(), startSample);
}

uint8_t HirariUnifiedEngine::complete_region_import(uint64_t ticket) {
    return hirari_region_import_complete(this, ticket);
}
