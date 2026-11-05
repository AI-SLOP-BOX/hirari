// History control (undo, redo, transactions, counts) is Rust-owned; the C++
// facade calls the Rust entry points directly. The one thing that must stay
// here is the graph resync that follows a replayed structural edit.
extern "C" void hirari_engine_sync_structural_changes(void* host) {
    if (host) static_cast<HirariUnifiedEngine*>(host)->syncStructuralChanges();
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


