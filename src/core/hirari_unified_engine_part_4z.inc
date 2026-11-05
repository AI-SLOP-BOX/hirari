// Region edit host bridge.
//
// The Rust region store owns the table, its undo capture and its snapshot
// publication; everything that used to reach into C++ region storage lives
// there now. What remains here is only the engine surface the store cannot
// own: the track registry, the handle lifetimes for imported audio and the
// pitch-analysis gate on the Track itself.

// Id resolution and the published snapshot both read the Rust track table now,
// so they live with the table accessors in the engine declaration.

size_t HirariUnifiedEngine::regionHostListTracks(void* host, uint32_t* out,
                                                 size_t capacity) {
    if (!host) return 0;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    size_t count = 0;
    for (void* track : engine->lockedTracks()) {
        if (!track) continue;
        if (out && count < capacity) out[count] = hirari_track_id(track);
        ++count;
    }
    return count;
}

void* HirariUnifiedEngine::regionHostTempoState(void* host) {
    return host ? static_cast<HirariUnifiedEngine*>(host)->m_tempoState : nullptr;
}

void* HirariUnifiedEngine::regionHostUndoState(void* host) {
    if (!host) return nullptr;
    return static_cast<HirariUnifiedEngine*>(host)->m_undoState;
}

double HirariUnifiedEngine::regionHostSampleRate(void* host) {
    if (!host) return 0.0;
    return hirari_engine_block_device_sample_rate(
        static_cast<HirariUnifiedEngine*>(host)->m_engineBlock);
}

size_t HirariUnifiedEngine::regionHostTrackIdentity(void* host, uint32_t trackId) {
    return withHostTrack(host, trackId, size_t{0},
                         [](void* track) { return reinterpret_cast<size_t>(track); });
}

void* HirariUnifiedEngine::regionHostRetainTrack(void* host, uint32_t trackId) {
    return host ? static_cast<HirariUnifiedEngine*>(host)->takeTrackOwner(trackId) : nullptr;
}

void HirariUnifiedEngine::regionHostReleaseTrack(void* handle) {
    hirari_track_owner_release(handle);
}

bool HirariUnifiedEngine::regionHostPitchBegin(void* trackOwner) {
    return hirari_track_pitch_begin(hirari_track_owner_object(trackOwner));
}

void HirariUnifiedEngine::regionHostPitchFinish(void* trackOwner) {
    hirari_track_pitch_finish(hirari_track_owner_object(trackOwner));
}

void* HirariUnifiedEngine::regionHostNoteSnapshot(void* trackOwner, uint32_t regionId) {
    return hirari_region_store_note_snapshot(hirari_track_owner_object(trackOwner), regionId);
}

namespace {

const HirariRegionHostOps kRegionEditHostOps = {
    nullptr,  // The Rust region store answers the region-data callbacks.
    nullptr, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr,
    &hirari_storage_handle_view,
    &hirari_storage_handle_release,
    nullptr,
    &HirariUnifiedEngine::regionHostListTracks,
    nullptr,
    &HirariUnifiedEngine::regionHostTempoState,
    &HirariUnifiedEngine::regionHostUndoState,
    &HirariUnifiedEngine::regionHostSampleRate,
    nullptr, nullptr,  // region_path and insert_region: Rust-owned.
    &HirariUnifiedEngine::regionHostTrackIdentity,
    &HirariUnifiedEngine::regionHostRetainTrack,
    &HirariUnifiedEngine::regionHostReleaseTrack,
    &HirariUnifiedEngine::regionHostPitchBegin,
    &HirariUnifiedEngine::regionHostPitchFinish,
    &HirariUnifiedEngine::regionHostNoteSnapshot,
    nullptr, nullptr,
};

}  // namespace
