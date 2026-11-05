
void HirariUnifiedEngine::prepareToPlay(double sr, uint32_t bs) {
    // A failed device negotiation must not advance the audio configuration
    // generation or partially replace the currently valid preparation state.
    // The rate/block bounds live in Rust (audio_config_rules.rs).
    if (!hirari_audio_format_valid(sr, bs)) return;
    // Close callback admission before waiting for in-flight callbacks. The
    // generation check alone cannot make master/track buffer reallocation
    // safe while a callback still traverses the previous buffers.
    hirari_engine_block_set_config_transition(handle(kHirariHandleEngineBlock), 1);
    struct ConfigTransitionGuard {
        void* state;
        ~ConfigTransitionGuard() { hirari_engine_block_set_config_transition(state, 0); }
    } transitionGuard{handle(kHirariHandleEngineBlock)};
    hirari_engine_block_join_callbacks(handle(kHirariHandleEngineBlock));
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);

        // Deferred Track objects are intentionally retained until shutdown.
        // prepareToPlay may race with a device callback that still has the
        // previous published snapshot, so transport state is not a reclamation
        // proof.
        hirari_engine_block_set_device_format(handle(kHirariHandleEngineBlock), sr, bs);
        hirari_engine_block_bump_generation(handle(kHirariHandleEngineBlock));
        hirari_engine_block_clear_midi_events(handle(kHirariHandleEngineBlock));

        hirari_master_limiter_prepare(handle(kHirariHandleMasterLimiter), sr);
        hirari_master_meter_prepare(handle(kHirariHandleMeter), sr);

        // One Rust walk over the track table: a freeze the new rate cannot
        // replay is dropped and retired with the layout marked, the track's
        // blocks and DSP retune, and a work block that reached the new length
        // re-offers its taps to the sidechain graph at this generation. Only
        // the allocation policy stays here because it is a C++ exception.
        const uint8_t status = hirari_engine_block_prepare_tracks(handle(kHirariHandleEngineBlock));
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
    hirari_engine_sync_structure(handle(kHirariHandleTrackTable), handle(kHirariHandleBusSystem), handle(kHirariHandleRouting), handle(kHirariHandlePdc),
                                 hirari_engine_block_generation(handle(kHirariHandleEngineBlock)));
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


void HirariUnifiedEngine::publishTrackSnapshotLocked(bool stampAudioGeneration) {
    // Rust fills the buffer the callback is not reading, then flips the index.
    hirari_engine_track_publish(
        handle(kHirariHandleTrackTable), hirari_engine_block_generation(handle(kHirariHandleEngineBlock)),
        stampAudioGeneration);
}

void HirariUnifiedEngine::syncNative(void* host) {
    if (host) static_cast<HirariUnifiedEngine*>(host)->syncStructuralChanges();
}

// The lifecycle walk itself is Rust's (track_edit_host.rs): it holds the tracks
// lock, builds each track through the Rust track object, moves the Rust track
// table, publishes the audio snapshot and replays the sidechain, route and
// routing captures through the shared track session. The engine contributes the
// stores it walks and the undo history it records into.
void HirariUnifiedEngine::installTrackEditHost() {
    static const HirariTrackEditHostOps ops = {
        &HirariUnifiedEngine::regionHostListTracks,
        &hirari_region_store_list_regions,
        [](void* host, uint32_t kind) -> void* {
            if (!host) return nullptr;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            switch (kind) {
                case kHirariTrackEditStoreTrackTable: return engine->handle(kHirariHandleTrackTable);
                case kHirariTrackEditStoreEngineBlock: return engine->handle(kHirariHandleEngineBlock);
                case kHirariTrackEditStoreSidechain: return engine->handle(kHirariHandleSidechainEngine);
                case kHirariTrackEditStoreSidechainRoutes: return engine->handle(kHirariHandleSidechainRoutes);
                case kHirariTrackEditStoreLayoutRevision:
                    return hirari_engine_block_layout_revision(engine->handle(kHirariHandleEngineBlock));
                case kHirariTrackEditStoreBusSystem: return engine->handle(kHirariHandleBusSystem);
                default: return engine->handle(kHirariHandleRouting);
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
