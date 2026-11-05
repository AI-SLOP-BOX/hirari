// History control (undo, redo, transactions, counts) is Rust-owned; the C++
// facade calls the Rust entry points directly. The one thing that must stay
// here is the graph resync that follows a replayed structural edit.
extern "C" void hirari_engine_sync_structural_changes(void* host) {
    if (host) static_cast<HirariUnifiedEngine*>(host)->syncStructuralChanges();
}

void* HirariUnifiedEngine::trackStorageHandle(void* track, uint32_t kind) noexcept {
    if (!track) return nullptr;
    auto* typed = static_cast<Track*>(track);
    switch (kind) {
        case 0: return typed->channelStripHandle();
        case 1: return typed->effectChainHandle();
        case 2: return typed->automationStoreHandle();
        case 3: return typed->frozenStoreHandle();
        case 4: return typed->realtimeRuntime();
        case 6: return typed->nameStoreHandle();
        case 7: return typed->workBufferStorage();
        default: return typed->articulationStateHandle();
    }
}

// Exposes the live tracks to the Rust layout gather (layout_gather.rs) and the
// shared track-edit session (track_session.rs). Every per-track field they need
// — faders, name, automation, plug-ins, freeze, regions — already lives in a
// Rust storage addressed through this function, so this table only holds the
// tracks lock and the track's id and type.
const HirariLayoutHostOps* HirariUnifiedEngine::layoutHostOps() const {
    static const HirariLayoutHostOps ops = {
        // Drain queued write-automation before taking the lock, so UI, CLI
        // generation checks, history diffs and saves observe one state.
        [](void* host) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            engine->m_tracksMutex.lock();
            for (Track* track : engine->lockedTracks()) {
                if (!track) continue;
                track->flushRecordedAutomation();
                track->reclaimEffectChainRetiredMemory();
            }
        },
        [](void* host) {
            static_cast<HirariUnifiedEngine*>(host)->m_tracksMutex.unlock();
        },
        [](void* host) {
            return static_cast<HirariUnifiedEngine*>(host)->lockedTracks().size();
        },
        [](void* host, size_t index) -> void* {
            return static_cast<HirariUnifiedEngine*>(host)->lockedTrackAt(index);
        },
        [](void* track) -> uint32_t {
            return track ? static_cast<Track*>(track)->getId() : 0u;
        },
        [](void* track) -> uint32_t {
            return track ? static_cast<uint32_t>(static_cast<Track*>(track)->getType()) : 0u;
        },
        &HirariUnifiedEngine::trackStorageHandle,
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_sidechainRoutes; },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_routingEngine; },
        [](void* host) {
            return static_cast<HirariUnifiedEngine*>(host)
                ->m_sampleRate.load(std::memory_order_relaxed);
        },
    };
    return &ops;
}

// The command admission check hashes the live layout, so this gathers and
// serializes in Rust and fingerprints the buffer without materialising a
// string. The background snapshot session lives entirely in Rust.
uint64_t HirariUnifiedEngine::get_project_generation() const {
    uint8_t* jsonData = nullptr;
    size_t jsonSize = 0;
    const bool written = hirari_layout_json_from_host(
        const_cast<HirariUnifiedEngine*>(this), layoutHostOps(),
        nullptr, nullptr, &jsonData, &jsonSize);
    uint64_t hash = 0xcbf29ce484222325ull;
    if (written && jsonData) {
        hirari_project_layout_fingerprint_update(&hash, jsonData, jsonSize);
    }
    hirari_project_layout_json_free(jsonData, jsonSize);
    return hash;
}

uint64_t HirariUnifiedEngine::get_project_layout_revision() const noexcept {
    return m_projectLayoutRevision ? m_projectLayoutRevision->current() : 0;
}

uint64_t HirariUnifiedEngine::get_project_state_revision() const noexcept {
    const uint64_t layout = get_project_layout_revision();
    const uint64_t midi = hirari_midi_snapshot_store_revision(m_midiSnapshotStore);
    const uint64_t control = m_projectControlRevision.load(std::memory_order_acquire);
    return hirari_project_state_revision_mix(layout, midi, control);
}

void HirariUnifiedEngine::installCommandHost() {
    static const HirariCommandHostOps ops = {
        [](void* host, uint32_t tid) -> bool {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            if (!engine) return false;
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            return engine->findTrackLocked(tid) != nullptr;
        },
        [](void* host, uint64_t* project, uint64_t* audio) -> bool {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            if (!engine) return false;
            *project = engine->get_project_generation();
            *audio = engine->get_audio_config_generation();
            return true;
        },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_engineRings; },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_forensicJournal; },
    };
    hirari_command_host_install(&ops);
}

void HirariUnifiedEngine::installTrackFreezeHost() {
    static const HirariTrackFreezeHostOps ops = {
        [](void* host, uint32_t tid, bool wait) -> void* {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            if (!engine) return nullptr;
            if (wait) engine->waitForAudioCallbacks();
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            return newTrackHandle(engine->findTrackSharedLocked(tid));
        },
        [](void* handle) { delete static_cast<std::shared_ptr<Track>*>(handle); },
        [](void* host) -> uint32_t {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            return engine ? engine->get_block_size() : 0;
        },
    };
    hirari_track_freeze_host_install(&ops);
}


