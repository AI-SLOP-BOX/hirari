// History control (undo, redo, transactions, counts) is Rust-owned; the C++
// facade calls the Rust entry points directly. The one thing that must stay
// here is the graph resync that follows a replayed structural edit.
extern "C" void hirari_engine_sync_structural_changes(void* host) {
    if (host) static_cast<HirariUnifiedEngine*>(host)->syncStructuralChanges();
}

// Exposes the live tracks to the Rust layout gather (layout_gather.rs) and the
// shared track-edit session (track_session.rs). The track and every storage it
// owns are Rust objects, so this table only holds the tracks lock, the graph
// window and the engine's own stores; identity, type, storage and the drained
// edits are the track's Rust entry points.
const HirariLayoutHostOps* HirariUnifiedEngine::layoutHostOps() const {
    static const HirariLayoutHostOps ops = {
        // Drain queued write-automation before taking the lock, so UI, CLI
        // generation checks, history diffs and saves observe one state.
        [](void* host) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            engine->m_tracksMutex.lock();
            for (void* track : engine->lockedTracks()) {
                if (!track) continue;
                hirari_track_flush_recorded_automation(track);
                hirari_track_reclaim_chain(track);
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
        &hirari_track_id,
        &hirari_track_type,
        &hirari_track_storage,
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
    return hirari_layout_revision_current(m_layoutRevision);
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
            *audio = hirari_engine_block_generation(engine->m_engineBlock);
            return true;
        },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_engineRings; },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->m_forensicJournal; },
    };
    hirari_command_host_install(&ops);
}

void HirariUnifiedEngine::installTrackFreezeHost() {
    static const HirariTrackFreezeHostOps ops = {
        // The owner handle is the Rust Arc's boxed clone, so it keeps the track
        // alive after the topology lock is released.
        [](void* host, uint32_t tid, bool wait) -> void* {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            if (!engine) return nullptr;
            if (wait) engine->waitForAudioCallbacks();
            return engine->takeTrackOwner(tid);
        },
        [](void* handle) { hirari_track_owner_release(handle); },
        [](void* host) -> uint32_t {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            return engine ? engine->get_block_size() : 0;
        },
    };
    hirari_track_freeze_host_install(&ops);
}


