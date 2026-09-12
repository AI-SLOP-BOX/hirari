            if (state.trackCount < kMaxTracks) state.activeTracks[state.trackCount++] = track.get();
        }
        m_activeStateIdx.store(nextIdx, std::memory_order_release);
    }

    const auto removeCopy = [this, newId]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
                               [newId](const auto& track) { return track->getId() == newId; });
        if (it == m_tracks.end()) return;
        {
            std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
            for (const auto& region : (*it)->getRegions()) m_peakCache.erase(region.id);
        }
        m_deferredDestructionTracks.push_back(*it);
        m_tracks.erase(it);
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
        uint32_t nextIdx = 1 - m_activeStateIdx.load(std::memory_order_acquire);
        auto& state = m_audioStates[nextIdx];
        state.trackCount = 0;
        for (const auto& track : m_tracks) {
            if (state.trackCount < kMaxTracks) state.activeTracks[state.trackCount++] = track.get();
        }
        m_activeStateIdx.store(nextIdx, std::memory_order_release);
    };
    m_undoManager.recordAppliedAction("Duplicate Track", removeCopy, restoreCopy);
    return newId;
}

bool AuraUnifiedEngine::remove_track(uint32_t tid) {
    const auto sidechainBackup = m_sidechainManager.captureLinksForTrack(tid);
    std::shared_ptr<Track> removed;
    size_t originalIndex = 0;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
                               [tid](const auto& track) { return track->getId() == tid; });
        if (it == m_tracks.end()) return false;
        originalIndex = static_cast<size_t>(it - m_tracks.begin());
        removed = *it;
        m_sidechainManager.removeLinksForTrack(tid);
        {
            std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
            for (const auto& region : removed->getRegions()) m_peakCache.erase(region.id);
        }
        m_deferredDestructionTracks.push_back(removed);
        m_tracks.erase(it);
        if (tid == 0 && m_tracks.empty()) {
            // new_project() creates the bootstrap at ID 0 and advances the
            // allocator. Removing that placeholder during project hydration
            // must rewind the allocator so the first persisted track retains
            // its canonical ID instead of becoming ID 2.
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
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        m_sidechainManager.removeLinksForTrack(tid);
        auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
                               [tid](const auto& track) { return track->getId() == tid; });
        if (it == m_tracks.end()) return;
        {
            std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
            for (const auto& region : (*it)->getRegions()) m_peakCache.erase(region.id);
        }
        m_deferredDestructionTracks.push_back(*it);
        m_tracks.erase(it);
        uint32_t nextIdx = 1 - m_activeStateIdx.load(std::memory_order_acquire);
        auto& state = m_audioStates[nextIdx];
        state.trackCount = 0;
        for (const auto& track : m_tracks) {
            if (state.trackCount < kMaxTracks) state.activeTracks[state.trackCount++] = track.get();
        }
        m_activeStateIdx.store(nextIdx, std::memory_order_release);
    };
    const auto restore = [this, removed, tid, originalIndex, sidechainBackup]() {
        {
            std::lock_guard<std::mutex> lock(m_tracksMutex);
            const bool alreadyPresent = std::any_of(m_tracks.begin(), m_tracks.end(),
                [tid](const auto& track) { return track->getId() == tid; });
            if (!alreadyPresent) {
                m_tracks.insert(m_tracks.begin() + std::min(originalIndex, m_tracks.size()), removed);
                uint32_t nextIdx = 1 - m_activeStateIdx.load(std::memory_order_acquire);
                auto& state = m_audioStates[nextIdx];
                state.trackCount = 0;
                for (const auto& track : m_tracks) {
                    if (state.trackCount < kMaxTracks) state.activeTracks[state.trackCount++] = track.get();
                }
                m_activeStateIdx.store(nextIdx, std::memory_order_release);
            }
        }
        m_sidechainManager.restoreLinks(sidechainBackup);
    };
    m_undoManager.recordAppliedAction("Remove Track", restore, removeAgain);
    return true;
}

bool AuraUnifiedEngine::set_track_name(uint32_t tid, std::string_view name) {
    if (name.empty() || name.size() > 256 || name.find('\0') != std::string_view::npos) return false;
    std::shared_ptr<Track> target;
    std::string before;
    const std::string nextName(name);
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() == tid) {
                target = track;
                before = track->getName();
                break;
            }
        }
    }
    if (!target) return false;
    // Naming is an idempotent project-hydration operation: restoring a name
    // that is already present still counts as success.
    if (before == nextName) return true;
    const auto apply = [this, target](const std::string& value) {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->setName(value);
    };
    apply(nextName);
    m_undoManager.recordAppliedAction(
        "Rename Track", [apply, before] { apply(before); }, [apply, nextName] { apply(nextName); });
    return true;
}

bool AuraUnifiedEngine::set_track_volume(uint32_t tid, float value) {
    if (!std::isfinite(value) || value < 0.0f || value > 2.0f) return false;
    std::shared_ptr<Track> target;
    float before = 1.0f;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track->getId() == tid) {
            target = track;
            before = track->getVolume();
            break;
        }
    }
    if (!target) return false;
    // Publish the control-thread state immediately. The undo transaction is
    // history bookkeeping; serialized project snapshots must not depend on
    // a later callback observing the redo closure.
    target->setVolume(value);
    const auto apply = [target](float next) { target->setVolume(next); };
    m_undoManager.performAction(
        "Track Volume " + std::to_string(tid),
        [apply, before] { apply(before); },
        [apply, value] { apply(value); });
    // Undo history is process-global while the engine is a singleton. A
    // coalesced transaction must never leave the live control state at its
    // captured pre-edit value after the API returns.
    target->setVolume(value);
    return true;
}

bool AuraUnifiedEngine::save_track_preset(std::string_view name, uint32_t track_id) {
    if (name.empty()) return false;
    std::shared_ptr<Track> t;
    { std::lock_guard<std::mutex> lock(m_tracksMutex); for (auto& x : m_tracks) if (x && x->getId() == track_id) { t = x; break; } }
    if (!t) return false;
    Aura::Core::Mixing::ChannelConfiguration c;
    c.faderDb = 20.0f * std::log10(std::max(1.0e-6f, t->getVolume()));
    c.pan = t->getPan(); c.mute = t->isMuted();
    return m_trackPresets.save(std::string(name), c);
}

bool AuraUnifiedEngine::apply_track_preset(std::string_view name, uint32_t track_id) {
    const auto* c = m_trackPresets.find(std::string(name)); if (!c) return false;
    std::shared_ptr<Track> t;
    { std::lock_guard<std::mutex> lock(m_tracksMutex); for (auto& x : m_tracks) if (x && x->getId() == track_id) { t = x; break; } }
    if (!t) return false;
    t->setVolume(std::clamp(std::pow(10.0f, c->faderDb / 20.0f), 0.0f, 2.0f)); t->setPan(c->pan); t->setMuted(c->mute); return true;
}

bool AuraUnifiedEngine::remove_track_preset(std::string_view name) { return m_trackPresets.remove(std::string(name)); }

float AuraUnifiedEngine::get_track_volume(uint32_t tid) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid) return track->getVolume();
    }
    return -1.0f;
}

bool AuraUnifiedEngine::set_track_pan(uint32_t tid, float value) {
    if (!std::isfinite(value) || value < -1.0f || value > 1.0f) return false;
    std::shared_ptr<Track> target;
    float before = 0.0f;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track->getId() == tid) {
            target = track;
            before = track->getPan();
            break;
        }
    }
    if (!target) return false;
    const auto apply = [target](float next) { target->setPan(next); };
    m_undoManager.performAction(
        "Track Pan " + std::to_string(tid),
        [apply, before] { apply(before); },
        [apply, value] { apply(value); });
    return true;
}

bool AuraUnifiedEngine::set_track_mute(uint32_t tid, bool muted) {
    std::shared_ptr<Track> target;
    bool before = false;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track->getId() == tid) {
            target = track;
            before = track->isMuted();
            break;
        }
    }
    if (!target) return false;
    const auto apply = [target](bool next) { target->setMuted(next); };
    m_undoManager.performAction(
        "Track Mute " + std::to_string(tid),
        [apply, before] { apply(before); },
        [apply, muted] { apply(muted); });
    return true;
}

bool AuraUnifiedEngine::set_track_solo(uint32_t tid, bool solo) {
    std::shared_ptr<Track> target;
    bool before = false;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track->getId() == tid) {
            target = track;
            before = track->isSolo();
            break;
        }
    }
    if (!target) return false;
    const auto apply = [target](bool next) { target->setSolo(next); };
    m_undoManager.performAction(
        "Track Solo " + std::to_string(tid),
        [apply, before] { apply(before); },
        [apply, solo] { apply(solo); });
    return true;
}

bool AuraUnifiedEngine::set_track_solo_for_offline_render(uint32_t tid, bool solo) {
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

bool AuraUnifiedEngine::set_offline_render_target(uint32_t tid) {
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

void AuraUnifiedEngine::clear_offline_render_target() noexcept {
    m_offlineTargetActive.store(false, std::memory_order_release);
    m_offlineTargetId.store(0, std::memory_order_release);
    m_offlineRenderTailMillis.store(0, std::memory_order_release);
    m_offlineRenderPreFader.store(false, std::memory_order_release);
    m_offlineRenderIncludeInserts.store(true, std::memory_order_release);
}

void AuraUnifiedEngine::set_offline_render_tail_seconds(float seconds) noexcept {
    if (!std::isfinite(seconds) || seconds < 0.0f) {
        m_offlineRenderTailMillis.store(0, std::memory_order_release);
        return;
    }
    const float bounded = std::min(seconds, 60.0f);
    m_offlineRenderTailMillis.store(
        static_cast<uint32_t>(bounded * 1000.0f), std::memory_order_release);
}

void AuraUnifiedEngine::set_offline_render_options(bool preFader, bool includeInserts) noexcept {
    m_offlineRenderPreFader.store(preFader, std::memory_order_release);
    m_offlineRenderIncludeInserts.store(includeInserts, std::memory_order_release);
}

bool AuraUnifiedEngine::set_offline_render_range(uint64_t startSample, uint64_t endSample) noexcept {
    if (startSample >= endSample) return false;
    m_offlineRenderStartSample.store(startSample, std::memory_order_release);
    m_offlineRenderEndSample.store(endSample, std::memory_order_release);
    m_offlineRenderRangeActive.store(true, std::memory_order_release);
    return true;
}

void AuraUnifiedEngine::clear_offline_render_range() noexcept {
    m_offlineRenderRangeActive.store(false, std::memory_order_release);
    m_offlineRenderStartSample.store(0, std::memory_order_release);
    m_offlineRenderEndSample.store(0, std::memory_order_release);
}

void AuraUnifiedEngine::prepareToPlay(double sr, uint32_t bs) {
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

void AuraUnifiedEngine::syncStructuralChanges() {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    
    // 1. Collect nodes for compilation
    std::vector<ProcessNode> nodes;
    for (auto& t : m_tracks) {
        ProcessNode n;
        n.type = ProcessNode::TrackNode;
        n.id = t->getId();
        n.ptr = t.get();
        // sidechain/bus dependencies can be discovered here
        nodes.push_back(n);
    }
    
    // 2. Compile Graph
        if (!m_processGraph) m_processGraph = std::make_unique<ProcessGraph>();
        m_processGraph->compile(nodes);
    
    // 3. Update Audio Thread State
    uint32_t nextIdx = 1 - m_activeStateIdx.load();
    auto& state = m_audioStates[nextIdx];
    state.audioConfigGeneration = m_audioConfigGeneration.load(std::memory_order_acquire);
    state.trackCount = 0;
    state.stageCount = 0;
    // INDUSTRIAL: Reset the O(1) track map.
    std::memset(state.trackMap, 0, sizeof(state.trackMap));
    
    const auto& stages = m_processGraph->getStages();
    for (const auto& stage : stages) {
        if (state.stageCount < 32) {
            state.stages[state.stageCount++] = stage;
        }
    }
    
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
    auto& pdc = m_pdcManager;
    auto& routing = m_routingEngine;
    for (const auto& track : m_tracks) {
        if (!track || track->getId() >= Aura::Core::Engine::PDCManager::kMaxTracks) continue;
        pdc.setTrackDest(track->getId(), Aura::Core::Engine::PDCManager::kMasterID);
        pdc.setTrackLatency(track->getId(), track->getTotalLatencySamples());
    }
    std::vector<Aura::Core::Engine::RoutingEngine::Connection> connections;
    routing.copyConnections(connections);
    for (const auto& connection : connections) {
        if (connection.sourceId < Aura::Core::Engine::PDCManager::kMaxTracks &&
            connection.destId < Aura::Core::Engine::PDCManager::kMaxTracks) {
            pdc.setTrackDest(connection.sourceId, connection.destId);
        }
    }
    pdc.recalculate();
    for (const auto& track : m_tracks) {
        if (track && track->getId() < Aura::Core::Engine::PDCManager::kMaxTracks) {
            track->setPdcCompensationSamples(pdc.getCompensationOffset(track->getId()));
        }
    }

    // --- BLACK BOX SYNC ---
    auto& bb = ::Aura::Core::Diagnostics::LogBuffer::BlackBoxRegister::getInstance();
    bb.lastTrackCount.store((uint32_t)m_tracks.size(), std::memory_order_relaxed);
    bb.lastSyncTimestamp.store(::Aura::Core::Diagnostics::LogBuffer::getTimestamp(), std::memory_order_relaxed);
    
    ::Aura::Core::Diagnostics::LogBuffer::post(0, 0x1, "Structural Sync: Topology Rebuilt.");
}

bool AuraUnifiedEngine::add_region(uint32_t tid, std::string_view path, double start_smp) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& t : m_tracks) {
        if (t->getId() == tid) {
            Region region{};
            region.id = allocateRegionIdLocked();
            if (region.id == 0) return false;
            region.path = std::string(path);
            region.start = static_cast<uint64_t>(std::max(0.0, start_smp));
            
            auto audioData = m_audioDecoderManager.importFile(region.path);
            if (!audioData || audioData->getNumSamples() == 0 || audioData->getNumChannels() == 0) {
                ::Aura::Core::Diagnostics::LogBuffer::post(1, 0x2101, "REGION_IMPORT_FAILED");
                return false;
            }
            region.audio = std::move(audioData);
            region.len = region.audio->getNumSamples();
            
            region.muted = false;
            region.name = region.path;
            const auto added = region;
            t->addRegion(region);
            // Region creation is an editing action: keep the imported audio
            // alive for redo while removing only this region on undo.
            m_undoManager.recordAppliedAction(
                "Add Region",
                [this, t, rid = added.id]() {
                    std::lock_guard<std::mutex> undoLock(m_tracksMutex);
                    t->removeRegion(rid);
                    std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
                    m_peakCache.erase(rid);
                },
                [this, t, added]() {
                    std::lock_guard<std::mutex> redoLock(m_tracksMutex);
                    t->removeRegion(added.id);
                    t->addRegion(added);
                    std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
                    m_peakCache.erase(added.id);
                });
            return true;
        }
    }
    ::Aura::Core::Diagnostics::LogBuffer::post(1, 0x2102, "REGION_TRACK_NOT_FOUND");
    return false;
}
