// History control (undo, redo, transactions, counts) is Rust-owned; the C++
// facade calls the Rust entry points directly. The one thing that must stay
// here is the graph resync that follows a replayed structural edit.
extern "C" void hirari_engine_sync_structural_changes(void* host) {
    if (host) static_cast<HirariUnifiedEngine*>(host)->syncStructuralChanges();
}

// Exposes the live tracks to the Rust layout gather (layout_gather.rs) and the
// shared track-edit session (track_session.rs). The gather resolves this engine
// through its session bag, reads the tracks lock above and fingerprints the
// layout itself, so the only thing left here is the token the command queue
// still echoes.
uint64_t HirariUnifiedEngine::get_project_generation() const {
    return hirari_project_generation(const_cast<HirariUnifiedEngine*>(this));
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
            *audio = hirari_engine_block_generation(engine->handle(kHirariHandleEngineBlock));
            return true;
        },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->handle(kHirariHandleEngineRings); },
        [](void* host) { return static_cast<HirariUnifiedEngine*>(host)->handle(kHirariHandleForensicJournal); },
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
            if (wait) hirari_engine_block_join_callbacks(engine->handle(kHirariHandleEngineBlock));
            return engine->takeTrackOwner(tid);
        },
        [](void* handle) { hirari_track_owner_release(handle); },
        [](void* host) -> uint32_t {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            return engine ? hirari_engine_block_device_block_frames(engine->handle(kHirariHandleEngineBlock)) : 0;
        },
    };
    hirari_track_freeze_host_install(&ops);
}


