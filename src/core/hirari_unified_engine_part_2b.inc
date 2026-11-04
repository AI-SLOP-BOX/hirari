
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

        // One Rust walk over the track table: a freeze the new rate cannot
        // replay is dropped and retired with the layout marked, the track's
        // blocks and DSP retune, and a work block that reached the new length
        // re-offers its taps to the sidechain graph at this generation. Only
        // the allocation policy stays here because it is a C++ exception.
        const uint8_t status = hirari_engine_block_prepare_tracks(m_engineBlock, sr, bs);
        if (status == 2) throw std::bad_array_new_length();
        if (status == 3) throw std::bad_alloc();
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

std::shared_ptr<Track> HirariUnifiedEngine::makeTrack(uint32_t id, const std::string& name,
                                                     Track::Type type) {
    if (type != Track::Type::Bus)
        return std::make_shared<Track>(id, name, type, m_projectLayoutRevision);
    auto bus = std::make_shared<BusTrack>(id, name, id, m_projectLayoutRevision);
    bus->resolveBus(hirari_bus_system_register(m_busSystem, id)
        ? hirari_bus_system_bus(m_busSystem, id) : nullptr);
    return bus;
}

void HirariUnifiedEngine::syncNative(void* host) {
    if (host) static_cast<HirariUnifiedEngine*>(host)->syncStructuralChanges();
}

// The lifecycle walk itself is Rust's (track_edit_host.rs): it holds the tracks
// lock, moves the Rust track table, publishes the audio snapshot and replays the
// sidechain, route and routing captures through the shared track session. Only
// building the C++ Track and naming the stores it walks stay here.
void HirariUnifiedEngine::installTrackEditHost() {
    static const HirariTrackEditHostOps ops = {
        &HirariUnifiedEngine::regionHostListTracks,
        &hirari_region_store_list_regions,
        [](void* host, uint32_t type, uint32_t id, const uint8_t* name, size_t nameLength,
           void** outTrack) -> void* {
            if (!host || !name || !outTrack) return nullptr;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            auto created = engine->makeTrack(
                id, std::string(reinterpret_cast<const char*>(name), nameLength),
                static_cast<Track::Type>(type));
            created->prepareToPlay(engine->m_sampleRate.load(std::memory_order_relaxed),
                                   engine->m_blockSize.load(std::memory_order_relaxed));
            *outTrack = created.get();
            return newTrackHandle(std::move(created));
        },
        [](void* host, uint32_t kind) -> void* {
            if (!host) return nullptr;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            switch (kind) {
                case kHirariTrackEditStoreTrackTable: return engine->m_trackTable;
                case kHirariTrackEditStoreEngineBlock: return engine->m_engineBlock;
                case kHirariTrackEditStoreSidechain: return engine->m_sidechainEngine;
                case kHirariTrackEditStoreSidechainRoutes: return engine->m_sidechainRoutes;
                default: return engine->m_routingEngine;
            }
        },
        nullptr,  // undo capture lives in the Rust track edit host.
        &HirariUnifiedEngine::syncNative,
        [](void* host) -> void* {
            return host ? static_cast<HirariUnifiedEngine*>(host)->undo_state() : nullptr;
        },
    };
    hirari_track_edit_host_install(&ops);
}
