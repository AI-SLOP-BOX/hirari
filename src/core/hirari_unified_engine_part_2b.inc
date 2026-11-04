

float HirariUnifiedEngine::get_track_volume(uint32_t tid) const {
    return queryLockedTrack(tid, -1.0f, [](const Track& track) { return track.getVolume(); });
}

bool HirariUnifiedEngine::set_offline_render_target(uint32_t tid) {
    // 0 is reserved for the master target. Track/bus IDs are validated
    // against the current graph before activation so a stale CLI target
    // cannot silently render an unrelated output.
    if (tid != 0) {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        if (!findTrackLocked(tid)) return false;
    }
    hirari_bounce_set_offline_target(m_bounceSession, tid);
    return true;
}

void HirariUnifiedEngine::clear_offline_render_target() {
    hirari_bounce_clear_offline_target(m_bounceSession);
}

void HirariUnifiedEngine::set_offline_render_tail_seconds(float seconds) {
    // Bounds and non-finite rejection are enforced by the bridge; the session
    // stores the millisecond value the render plan consumes.
    hirari_bounce_set_tail_seconds(m_bounceSession, seconds);
}

void HirariUnifiedEngine::set_offline_render_options(bool preFader, bool includeInserts) {
    hirari_bounce_set_render_options(m_bounceSession, preFader, includeInserts);
}

void HirariUnifiedEngine::prepareToPlay(double sr, uint32_t bs) {
    // A failed device negotiation must not advance the audio configuration
    // generation or partially replace the currently valid preparation state.
    // The rate/block bounds live in Rust (audio_config_rules.rs).
    if (!hirari_audio_format_valid(sr, bs)) return;
    // Close callback admission before waiting for in-flight callbacks. The
    // generation check alone cannot make master/track buffer reallocation
    // safe while a callback still traverses the previous buffers.
    hirari_engine_block_set_config_transition(m_engineBlock, 1);
    struct ConfigTransitionGuard {
        void* state;
        ~ConfigTransitionGuard() { hirari_engine_block_set_config_transition(state, 0); }
    } transitionGuard{m_engineBlock};
    waitForAudioCallbacks();
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);

        // Deferred Track objects are intentionally retained until shutdown.
        // prepareToPlay may race with a device callback that still has the
        // previous published snapshot, so transport state is not a reclamation
        // proof.
        m_sampleRate.store(sr);
        hirari_engine_block_bump_generation(m_engineBlock);
        hirari_engine_block_clear_midi_events(m_engineBlock);
        m_blockSize.store(bs, std::memory_order_relaxed);

        hirari_master_limiter_prepare(m_masterLimiter, sr);
        hirari_master_meter_prepare(m_meterCore, sr);

        for (Track* t : lockedTracks()) {
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
                void* work = t->workBufferStorage();
                const uint32_t channels = hirari_audio_buffer_storage_channels(work);
                hirari_sidechain_engine_refresh_source(
                    m_sidechainEngine, t->getId(),
                    hirari_audio_buffer_storage_read_pointer(work, 0, 0),
                    channels > 1 ? hirari_audio_buffer_storage_read_pointer(work, 1, 0)
                                 : hirari_audio_buffer_storage_read_pointer(work, 0, 0),
                    bs, static_cast<uint32_t>(sr),
                    hirari_engine_block_generation(m_engineBlock));
            }
        }
    }
    // syncStructuralChanges() acquires m_tracksMutex itself. Never call it
    // while the preparation lock is held.
    syncStructuralChanges();
}

void HirariUnifiedEngine::syncStructuralChanges() {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    // One ordered control walk, owned by Rust: the bus slots the graph occupies,
    // the routing rebuild, the snapshot publication and the solved PDC offsets.
    // The engine supplies the per-track scalars and applies the result.
    hirari_engine_sync_structure(m_trackTable, m_busSystem, m_routingEngine, m_pdcState,
                                 hirari_engine_block_generation(m_engineBlock));
    hirari_log_post(0, 0x1, "Structural Sync: Topology Rebuilt.");
}

// Region import policy (validation, decoder dispatch, ticket limits, worker
// thread, undo capture) lives in Rust (region_jobs.rs). These wrappers only
// translate the result codes into the existing UI diagnostics.
bool HirariUnifiedEngine::add_region(uint32_t tid, std::string_view path, double start_smp) {
    const int32_t result = hirari_region_import_add(
        this, tid, reinterpret_cast<const uint8_t*>(path.data()), path.size(), start_smp);
    if (result == 1) {
        hirari_log_post(1, 0x2102, "REGION_TRACK_NOT_FOUND");
    } else if (result == 2) {
        hirari_log_post(1, 0x2101, "REGION_IMPORT_FAILED");
    }
    return result == 0;
}

// The Rust table owns the graph, so it only needs these reads from C++: the id
// behind a Track*, the copy/free of the heap shared_ptr it stores, and the four
// scalars its structural walk solves over.
const HirariEngineTrackOps* HirariUnifiedEngine::trackOps() noexcept {
    static const HirariEngineTrackOps ops = {
        [](void* track) -> uint32_t { return static_cast<const Track*>(track)->getId(); },
        [](void* owner) -> void* {
            return new std::shared_ptr<Track>(
                *static_cast<std::shared_ptr<Track>*>(owner));
        },
        [](void* owner) { delete static_cast<std::shared_ptr<Track>*>(owner); },
        [](void* track) -> uint32_t {
            return static_cast<uint32_t>(static_cast<const Track*>(track)->getType());
        },
        [](void* track) -> uint32_t {
            const auto* bus = dynamic_cast<const BusTrack*>(static_cast<const Track*>(track));
            return bus ? bus->busId() : UINT32_MAX;
        },
        [](void* track) -> uint32_t {
            return static_cast<const Track*>(track)->getTotalLatencySamples();
        },
        [](void* track, uint32_t samples) {
            static_cast<Track*>(track)->setPdcCompensationSamples(samples);
        },
    };
    return &ops;
}

void HirariUnifiedEngine::publishTrackSnapshotLocked(bool stampAudioGeneration) {
    // Rust fills the buffer the callback is not reading, then flips the index.
    hirari_engine_track_publish(
        m_trackTable, hirari_engine_block_generation(m_engineBlock),
        stampAudioGeneration);
}

std::shared_ptr<Track> HirariUnifiedEngine::detachTrackLocked(uint32_t tid, bool stampGeneration) {
    Track* detached = findTrackLocked(tid);
    if (!detached) return nullptr;
    hirari_region_store_forget_waveforms(this, detached);
    // The table keeps its own reference in the deferred list, so the caller gets
    // a second one while a device callback may still hold the raw pointer.
    auto owned = adoptTrackHandle(hirari_engine_track_detach(m_trackTable, tid));
    if (!owned) return nullptr;
    mark_project_layout_changed();
    publishTrackSnapshotLocked(stampGeneration);
    return owned;
}

std::shared_ptr<Track> HirariUnifiedEngine::makeTrack(uint32_t id, const std::string& name,
                                                     Track::Type type) {
    if (type != Track::Type::Bus)
        return std::make_shared<Track>(id, name, type, m_projectLayoutRevision);
    auto bus = std::make_shared<BusTrack>(id, name, id, m_projectLayoutRevision);
    bus->resolveBus(hirari_bus_system_register(m_busSystem, id)
        ? hirari_bus_system_bus(m_busSystem, id) : nullptr);
    return bus;
}

void HirariUnifiedEngine::insertTrackLocked(const std::shared_ptr<Track>& track, size_t index,
                                            bool stampGeneration) {
    // Restoring an id that is already in the graph is a no-op, as it was when the
    // vector lived here; the table clamps an out-of-range index to the end.
    if (!track || findTrackLocked(track->getId())) return;
    Track* raw = track.get();
    if (!hirari_engine_track_insert(m_trackTable, index, newTrackHandle(track), raw)) return;
    mark_project_layout_changed();
    publishTrackSnapshotLocked(stampGeneration);
}

void HirariUnifiedEngine::syncNative(void* host) {
    if (host) static_cast<HirariUnifiedEngine*>(host)->syncStructuralChanges();
}

void HirariUnifiedEngine::installTrackEditHost() {
    struct Handle {
        std::shared_ptr<Track> track;
        size_t index = 0;
        uint32_t id = 0;
        void* sidechainLinks = nullptr;
        void* sidechainRoutes = nullptr;
        std::vector<RoutingEngine::Connection> routing;
        std::vector<RoutingEngine::FeedbackConnection> feedback;
    };
    static const HirariTrackEditHostOps ops = {
        &HirariUnifiedEngine::regionHostListTracks,
        &hirari_region_store_list_regions,
        [](void* host, uint32_t type, uint32_t id, const uint8_t* name, size_t nameLength,
           size_t* outIndex) -> void* {
            if (!host || !name || !outIndex) return nullptr;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            const std::string trackName(reinterpret_cast<const char*>(name), nameLength);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            auto created = engine->makeTrack(id, trackName, static_cast<Track::Type>(type));
            created->prepareToPlay(engine->m_sampleRate.load(std::memory_order_relaxed),
                                   engine->m_blockSize.load(std::memory_order_relaxed));
            Track* raw = created.get();
            const size_t appendIndex = engine->lockedTracks().size();
            (void)hirari_engine_track_insert(engine->m_trackTable, appendIndex,
                                           newTrackHandle(std::move(created)), raw);
            auto* handle = new Handle{engine->findTrackSharedLocked(id), appendIndex, id};
            engine->mark_project_layout_changed();
            engine->publishTrackSnapshotLocked(true);
            *outIndex = handle->index;
            return handle;
        },
        [](void* host, uint32_t tid, uint32_t newId, const uint32_t* regionIds,
           size_t regionIdCount, size_t* outIndex) -> void* {
            if (!host || !outIndex || (!regionIds && regionIdCount != 0)) return nullptr;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::shared_ptr<Track> copy;
            size_t insertIndex = 0;
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
                Track* original = engine->findTrackLocked(tid);
                if (!original) return nullptr;
                if (hirari_region_store_count(original) != regionIdCount) return nullptr;
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
                if (!hirari_region_store_copy_regions(original, copy.get(), regionIds, regionIdCount))
                    return nullptr;
                insertIndex = engine->lockedTracks().size();
                (void)hirari_engine_track_insert(engine->m_trackTable, insertIndex,
                                               newTrackHandle(copy), copy.get());
                engine->mark_project_layout_changed();
                engine->publishTrackSnapshotLocked(false);
            }
            auto* handle = new Handle{copy, insertIndex, newId};
            *outIndex = insertIndex;
            return handle;
        },
        [](void* host, uint32_t tid) -> void* {
            if (!host) return nullptr;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            void* sidechainBackup = hirari_sidechain_engine_capture_links(
                engine->m_sidechainEngine, tid);
            std::shared_ptr<Track> removed;
            void* sidechainRouteBackup = nullptr;
            std::vector<RoutingEngine::Connection> routingBackup;
            std::vector<RoutingEngine::FeedbackConnection> feedbackBackup;
            size_t originalIndex = 0;
            {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                const size_t foundIndex = hirari_engine_track_index_of(engine->m_trackTable, tid);
                if (foundIndex == std::numeric_limits<size_t>::max()) return nullptr;
                originalIndex = foundIndex;
                hirari_sidechain_engine_remove_track(engine->m_sidechainEngine, tid);
                sidechainRouteBackup =
                    hirari_sidechain_routes_capture_track(engine->m_sidechainRoutes, tid);
                void* routingHandle = engine->m_routingEngine;
                hirari_routing_engine_remove_dependencies_for_node(routingHandle, tid);
                size_t routingCount = 0;
                size_t feedbackCount = 0;
                void* removedRouting = hirari_routing_engine_remove_connections_for_node(
                    routingHandle, tid, &routingCount);
                void* removedFeedback = hirari_routing_engine_remove_feedback_for_node(
                    routingHandle, tid, &feedbackCount);
                routingBackup = takeRoutingConnections(removedRouting, routingCount);
                feedbackBackup = takeRoutingFeedback(removedFeedback, feedbackCount);
                removed = engine->detachTrackLocked(tid, false);
            }
            return new Handle{std::move(removed), originalIndex, tid, sidechainBackup,
                              sidechainRouteBackup, std::move(routingBackup),
                              std::move(feedbackBackup)};
        },
        [](void* host, void* raw, bool dropSidechainLinks, bool dropRouting) -> bool {
            if (!host || !raw) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            auto* handle = static_cast<Handle*>(raw);
            {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                if (dropSidechainLinks)
                    hirari_sidechain_engine_remove_track(engine->m_sidechainEngine, handle->id);
                if (dropRouting) {
                    hirari_sidechain_routes_erase_track(engine->m_sidechainRoutes, handle->id);
                    hirari_routing_engine_remove_dependencies_for_node(
                        engine->m_routingEngine, handle->id);
                }
                engine->detachTrackLocked(handle->id, false);
            }
            if (!dropRouting) return true;
            size_t discardedRouteCount = 0;
            size_t discardedFeedbackCount = 0;
            void* discardedRouteRecords = hirari_routing_engine_remove_connections_for_node(
                engine->m_routingEngine, handle->id, &discardedRouteCount);
            void* discardedFeedbackRecords = hirari_routing_engine_remove_feedback_for_node(
                engine->m_routingEngine, handle->id, &discardedFeedbackCount);
            takeRoutingConnections(discardedRouteRecords, discardedRouteCount);
            takeRoutingFeedback(discardedFeedbackRecords, discardedFeedbackCount);
            return true;
        },
        [](void* host, void* raw, bool stampGeneration) -> bool {
            if (!host || !raw) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            auto* handle = static_cast<Handle*>(raw);
            {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                engine->insertTrackLocked(handle->track, handle->index, stampGeneration);
                hirari_sidechain_routes_restore(engine->m_sidechainRoutes,
                                                engine->m_routingEngine,
                                                handle->sidechainRoutes);
            }
            (void)hirari_sidechain_engine_restore_links(engine->m_sidechainEngine,
                                                        handle->sidechainLinks);
            for (const auto& connection : handle->routing)
                hirari_routing_engine_add_connection(
                    engine->m_routingEngine, connection.sourceId, connection.destId,
                    connection.gain, connection.send, connection.preFader);
            for (const auto& connection : handle->feedback)
                hirari_routing_engine_add_feedback(
                    engine->m_routingEngine, connection.sourceId, connection.destId,
                    connection.gain);
            return true;
        },
        [](void* raw) {
            auto* handle = static_cast<Handle*>(raw);
            hirari_sidechain_engine_free_captured_links(handle->sidechainLinks);
            hirari_sidechain_routes_release(handle->sidechainRoutes);
            delete handle;
        },
        nullptr,  // undo capture lives in the Rust track edit host.
        &HirariUnifiedEngine::syncNative,
        [](void* host) -> void* {
            return host ? static_cast<HirariUnifiedEngine*>(host)->undo_state() : nullptr;
        },
    };
    hirari_track_edit_host_install(&ops);
}
