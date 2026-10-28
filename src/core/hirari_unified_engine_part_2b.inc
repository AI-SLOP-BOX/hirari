bool HirariUnifiedEngine::remove_track(uint32_t tid) {
    return hirari_track_remove(this, tid);
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
    // Bounds and non-finite rejection are enforced by the bridge; this only
    // stores the value the render loop consumes.
    m_offlineRenderTailMillis.store(
        static_cast<uint32_t>(seconds * 1000.0f), std::memory_order_release);
}

void HirariUnifiedEngine::set_offline_render_options(bool preFader, bool includeInserts) noexcept {
    m_offlineRenderPreFader.store(preFader, std::memory_order_release);
    m_offlineRenderIncludeInserts.store(includeInserts, std::memory_order_release);
}

void HirariUnifiedEngine::prepareToPlay(double sr, uint32_t bs) {
    // A failed device negotiation must not advance the audio configuration
    // generation or partially replace the currently valid preparation state.
    // The rate/block bounds live in Rust (audio_config_rules.rs).
    if (!hirari_audio_format_valid(sr, bs)) return;
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

void HirariUnifiedEngine::publishTrackSnapshotLocked(bool stampAudioGeneration) {
    uint32_t nextIdx = 1 - m_activeStateIdx.load(std::memory_order_acquire);
    auto& state = m_audioStates[nextIdx];
    if (stampAudioGeneration)
        state.audioConfigGeneration = m_audioConfigGeneration.load(std::memory_order_acquire);
    state.trackCount = 0;
    for (const auto& track : m_tracks) {
        if (state.trackCount < kMaxTracks) state.activeTracks[state.trackCount++] = track.get();
    }
    m_activeStateIdx.store(nextIdx, std::memory_order_release);
}

std::shared_ptr<Track> HirariUnifiedEngine::detachTrackLocked(uint32_t tid, bool stampGeneration) {
    auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
                           [tid](const auto& track) { return track->getId() == tid; });
    if (it == m_tracks.end()) return nullptr;
    auto detached = *it;
    for (const auto& region : detached->getRegions()) {
        hirari_region_waveform_forget(this, region.id);
    }
    m_deferredDestructionTracks.push_back(detached);
    m_tracks.erase(it);
    mark_project_layout_changed();
    publishTrackSnapshotLocked(stampGeneration);
    return detached;
}

void HirariUnifiedEngine::eraseSidechainRoutesLocked(uint32_t tid,
                                                    std::vector<SidechainRoute>* removed) {
    for (const auto& route : m_sidechainRoutes) {
        if ((route.sourceId == tid || route.destinationId == tid) && removed)
            removed->push_back(route);
    }
    m_sidechainRoutes.erase(
        std::remove_if(m_sidechainRoutes.begin(), m_sidechainRoutes.end(),
            [tid](const SidechainRoute& route) {
                return route.sourceId == tid || route.destinationId == tid;
            }),
        m_sidechainRoutes.end());
}

std::shared_ptr<Track> HirariUnifiedEngine::makeTrack(uint32_t id, const std::string& name,
                                                     Track::Type type) {
    if (type != Track::Type::Bus)
        return std::make_shared<Track>(id, name, type, &m_sidechainManager, m_projectLayoutRevision);
    auto bus = std::make_shared<BusTrack>(id, name, id, &m_sidechainManager, m_projectLayoutRevision);
    bus->resolveBus(m_busSystem.registerBus(id));
    return bus;
}

void HirariUnifiedEngine::insertTrackLocked(const std::shared_ptr<Track>& track, size_t index,
                                            bool stampGeneration) {
    const bool alreadyPresent = std::any_of(m_tracks.begin(), m_tracks.end(),
        [&track](const auto& current) { return current->getId() == track->getId(); });
    if (alreadyPresent) return;
    m_tracks.insert(m_tracks.begin() + std::min(index, m_tracks.size()), track);
    mark_project_layout_changed();
    publishTrackSnapshotLocked(stampGeneration);
}

void HirariUnifiedEngine::recordNativeReplay(void* host, const uint8_t* label, size_t labelLength,
                                             void* edit, ReplayCallback undo,
                                             ReplayCallback redo, ReplayCallback release) {
    if (!host || !edit) return;
    // Both history callbacks share the edit; it is released when the entry is
    // dropped. Replay re-enters Rust, which owns what undo and redo do.
    std::shared_ptr<void> owned(edit, [release](void* handle) { release(handle); });
    const std::string name(reinterpret_cast<const char*>(label), labelLength);
    static_cast<HirariUnifiedEngine*>(host)->m_undoManager.recordAppliedAction(
        name, [owned, undo]() { undo(owned.get()); }, [owned, redo]() { redo(owned.get()); });
}

void HirariUnifiedEngine::syncNative(void* host) {
    if (host) static_cast<HirariUnifiedEngine*>(host)->syncStructuralChanges();
}

void HirariUnifiedEngine::installTrackEditHost() {
    struct Handle {
        std::shared_ptr<Track> track;
        size_t index = 0;
        uint32_t id = 0;
        std::vector<SidechainManager::LinkState> sidechainLinks;
        std::vector<SidechainRoute> sidechainRoutes;
        std::vector<RoutingEngine::Connection> routing;
        std::vector<RoutingEngine::FeedbackConnection> feedback;
    };
    static const HirariTrackEditHostOps ops = {
        [](void* host) -> uint32_t {
            if (!host) return 0;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            return engine->allocateTrackIdLocked();
        },
        [](void* host, uint32_t type, uint32_t id, const uint8_t* name, size_t nameLength,
           size_t* outIndex) -> void* {
            if (!host || !name || !outIndex) return nullptr;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            const std::string trackName(reinterpret_cast<const char*>(name), nameLength);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            auto created = engine->makeTrack(id, trackName, static_cast<Track::Type>(type));
            if (engine->m_context)
                created->prepareToPlay(engine->m_context->sampleRate, engine->m_context->blockSize);
            auto* handle = new Handle{created, engine->m_tracks.size(), id, {}, {}, {}, {}};
            engine->m_tracks.push_back(std::move(created));
            engine->mark_project_layout_changed();
            engine->publishTrackSnapshotLocked(true);
            *outIndex = handle->index;
            return handle;
        },
        [](void* host, uint32_t tid, uint32_t* outId, size_t* outIndex) -> void* {
            if (!host || !outId || !outIndex) return nullptr;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::shared_ptr<Track> copy;
            size_t insertIndex = 0;
            uint32_t newId = 0;
            std::vector<uint32_t> pluginTypes;
            std::vector<std::vector<uint8_t>> pluginStates;
            std::vector<bool> pluginBypasses;
            std::vector<std::string> sandboxPaths;
            std::vector<ProjectAutomationPoint> volumeAutomation;
            std::vector<ProjectAutomationPoint> panAutomation;
            std::vector<ProjectAutomationPoint> delayAutomation;
            std::vector<ProjectPluginAutomationLane> pluginAutomation;
            {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                auto source = std::find_if(engine->m_tracks.begin(), engine->m_tracks.end(),
                                           [tid](const auto& track) { return track->getId() == tid; });
                if (source == engine->m_tracks.end()) return nullptr;
                const auto& original = *source;
                pluginTypes = original->getPluginTypes();
                pluginStates = original->getPluginStates();
                volumeAutomation = original->getVolumeAutomationPoints();
                panAutomation = original->getPanAutomationPoints();
                delayAutomation = original->getTrackDelayAutomationPoints();
                pluginAutomation = original->getPluginAutomation();
                pluginBypasses.reserve(pluginTypes.size());
                for (size_t index = 0; index < pluginTypes.size(); ++index) {
                    pluginBypasses.push_back(original->getPluginBypass(static_cast<uint32_t>(index)));
                }
                sandboxPaths = original->getSandboxedPluginPaths();
                newId = engine->allocateTrackIdLocked();
                if (newId == 0) return nullptr;
                copy = engine->makeTrack(newId, original->getName() + " Copy",
                                         original->getType());
                copy->setVolume(original->getVolume());
                copy->setPan(original->getPan());
                copy->setMuted(original->isMuted());
                copy->setPhaseInverted(original->isPhaseInverted());
                copy->setVolumeAutomation(volumeAutomation);
                copy->setPanAutomation(panAutomation);
                copy->setTrackDelayAutomation(delayAutomation);
                size_t sandboxPathIndex = 0;
                for (size_t pluginIndex = 0; pluginIndex < pluginTypes.size(); ++pluginIndex) {
                    const uint32_t pluginType = pluginTypes[pluginIndex];
                    bool inserted = false;
                    if (pluginType == 0xFFFFFFFFu) {
                        if (sandboxPathIndex < sandboxPaths.size()) {
                            inserted = copy->addSandboxedPlugin(sandboxPaths[sandboxPathIndex]);
                        }
                        ++sandboxPathIndex;
                    } else {
                        inserted = copy->addPlugin(pluginType);
                    }
                    if (inserted && pluginIndex < pluginStates.size() &&
                        !pluginStates[pluginIndex].empty()) {
                        const uint32_t destinationIndex =
                            static_cast<uint32_t>(copy->getPluginCount() - 1);
                        copy->setPluginState(destinationIndex, pluginStates[pluginIndex]);
                        if (pluginIndex < pluginBypasses.size()) {
                            copy->setPluginBypass(destinationIndex, pluginBypasses[pluginIndex]);
                        }
                    }
                }
                if (!pluginAutomation.empty()) copy->setPluginAutomation(pluginAutomation);
                for (const auto& region : original->getRegions()) {
                    Region cloned = region;
                    cloned.id = engine->allocateRegionIdLocked();
                    if (cloned.id == 0) return nullptr;
                    copy->addRegion(cloned);
                }
                // MIDI regions are independent objects and cannot be shallow-copied
                // because their note vectors are mutex-protected. Clone the notes
                // through the safe control-plane accessor and allocate fresh IDs.
                for (const auto& midiRegion : original->getMidiRegions()) {
                    if (!midiRegion) continue;
                    std::vector<MIDINote> notes;
                    if (!midiRegion->copyProcessedNotes(notes)) return nullptr;
                    const uint32_t midiId = engine->allocateRegionIdLocked();
                    if (midiId == 0) return nullptr;
                    auto midiCopy = std::make_shared<Hirari::Core::MidiRegion>(
                        midiId, midiRegion->getName(), midiRegion->getStartBeat(),
                        midiRegion->getLengthBeats());
                    if (!midiCopy->replaceNotes(notes)) return nullptr;
                    copy->addMidiRegion(std::move(midiCopy));
                }
                insertIndex = engine->m_tracks.size();
                engine->m_tracks.push_back(copy);
                engine->mark_project_layout_changed();
                engine->publishTrackSnapshotLocked(false);
            }
            auto* handle = new Handle{copy, insertIndex, newId, {}, {}, {}, {}};
            *outId = newId;
            *outIndex = insertIndex;
            return handle;
        },
        [](void* host, uint32_t tid) -> void* {
            if (!host) return nullptr;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            auto sidechainBackup = engine->m_sidechainManager.captureLinksForTrack(tid);
            std::shared_ptr<Track> removed;
            std::vector<SidechainRoute> sidechainRouteBackup;
            std::vector<RoutingEngine::Connection> routingBackup;
            std::vector<RoutingEngine::FeedbackConnection> feedbackBackup;
            size_t originalIndex = 0;
            {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                auto it = std::find_if(engine->m_tracks.begin(), engine->m_tracks.end(),
                                       [tid](const auto& track) { return track->getId() == tid; });
                if (it == engine->m_tracks.end()) return nullptr;
                originalIndex = static_cast<size_t>(it - engine->m_tracks.begin());
                engine->m_sidechainManager.removeLinksForTrack(tid);
                engine->eraseSidechainRoutesLocked(tid, &sidechainRouteBackup);
                engine->m_routingEngine.removeProcessingDependenciesForNode(tid);
                engine->m_routingEngine.removeConnectionsForNode(tid, routingBackup);
                engine->m_routingEngine.removeFeedbackConnectionsForNode(tid, feedbackBackup);
                removed = engine->detachTrackLocked(tid, false);
                if (tid == 1 && engine->m_tracks.empty()) {
                    // new_project() creates the bootstrap at ID 1 and advances the
                    // allocator to 2. Removing that placeholder during project
                    // hydration must rewind the allocator so the first persisted track
                    // keeps its canonical ID instead of becoming ID 2.
                    engine->m_trackIdCounter.store(1, std::memory_order_release);
                }
            }
            return new Handle{std::move(removed), originalIndex, tid,
                              std::move(sidechainBackup), std::move(sidechainRouteBackup),
                              std::move(routingBackup), std::move(feedbackBackup)};
        },
        [](void* host, void* raw, bool dropSidechainLinks, bool dropRouting) -> bool {
            if (!host || !raw) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            auto* handle = static_cast<Handle*>(raw);
            {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                if (dropSidechainLinks) engine->m_sidechainManager.removeLinksForTrack(handle->id);
                if (dropRouting) {
                    engine->eraseSidechainRoutesLocked(handle->id, nullptr);
                    engine->m_routingEngine.removeProcessingDependenciesForNode(handle->id);
                }
                engine->detachTrackLocked(handle->id, false);
            }
            if (!dropRouting) return true;
            std::vector<RoutingEngine::Connection> discardedRoutes;
            engine->m_routingEngine.removeConnectionsForNode(handle->id, discardedRoutes);
            std::vector<RoutingEngine::FeedbackConnection> discardedFeedback;
            engine->m_routingEngine.removeFeedbackConnectionsForNode(handle->id, discardedFeedback);
            return true;
        },
        [](void* host, void* raw, bool stampGeneration) -> bool {
            if (!host || !raw) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            auto* handle = static_cast<Handle*>(raw);
            {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                engine->insertTrackLocked(handle->track, handle->index, stampGeneration);
                for (const auto& route : handle->sidechainRoutes) {
                    const auto duplicate = std::find_if(
                        engine->m_sidechainRoutes.begin(), engine->m_sidechainRoutes.end(),
                        [&route](const SidechainRoute& current) {
                            return current.destinationId == route.destinationId &&
                                   current.pluginIndex == route.pluginIndex;
                        });
                    if (duplicate == engine->m_sidechainRoutes.end()) {
                        if (engine->m_routingEngine.addProcessingDependency(
                                route.sourceId, route.destinationId)) {
                            engine->m_sidechainRoutes.push_back(route);
                        }
                    }
                }
            }
            engine->m_sidechainManager.restoreLinks(handle->sidechainLinks);
            for (const auto& connection : handle->routing)
                engine->m_routingEngine.addConnection(connection.sourceId, connection.destId,
                                                      connection.gain, connection.send,
                                                      connection.preFader);
            for (const auto& connection : handle->feedback)
                engine->m_routingEngine.addFeedbackConnection(connection.sourceId,
                                                              connection.destId, connection.gain);
            return true;
        },
        [](void* raw) { delete static_cast<Handle*>(raw); },
        &HirariUnifiedEngine::recordNativeReplay,
        &HirariUnifiedEngine::syncNative,
    };
    hirari_track_edit_host_install(&ops);
}

namespace {
[[maybe_unused]] const bool kTrackEditHostInstalled = [] {
    HirariUnifiedEngine::installTrackEditHost();
    return true;
}();
}  // namespace
