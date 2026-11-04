// Pitch analysis is a Rust job (region_jobs.rs): rate validation, the worker,
// its registry limits, the staleness check and the undo pair all live there.
// The engine only exposes the retained-track storage callbacks (part_4z).

// Alignment and sync-group quantize are Rust jobs: validation, the job
// registry, analysis threads, staleness checks and the marker commit all live
// in hirari-core-bridge/src/region_time_jobs.rs. The engine only provides the
// host callbacks (part_4z) that expose storage.

bool HirariUnifiedEngine::append_region_processing(uint32_t tid, uint32_t rid, uint32_t stepId, std::string_view operation, float parameter, bool enabled) {
    const HirariEventStep step{stepId, reinterpret_cast<const uint8_t*>(operation.data()),
                               operation.size(), parameter, enabled};
    Track* track = findPublishedTrack(tid);
    return track != nullptr && hirari_region_store_append_processing(track, rid, step);
}
bool HirariUnifiedEngine::clear_region_processing(uint32_t tid, uint32_t rid) {
    Track* track = findPublishedTrack(tid);
    return track != nullptr && hirari_region_store_clear_processing(track, rid);
}

bool HirariUnifiedEngine::loadProject(std::string_view path) {
    bool expectedHydration = false;
    if (!m_projectHydrationActive.compare_exchange_strong(
            expectedHydration, true, std::memory_order_acq_rel,
            std::memory_order_acquire)) {
        return false;
    }
    struct HydrationGuard {
        std::atomic<bool>& active;
        ~HydrationGuard() { active.store(false, std::memory_order_release); }
    } hydrationGuard{m_projectHydrationActive};

    const std::filesystem::path projectPath(path);
    const std::filesystem::path projectDir = projectPath.has_parent_path()
        ? projectPath.parent_path()
        : std::filesystem::current_path();
    auto state = ::Hirari::Core::Bridge::native_project_load(
        rust::Str(path.data(), path.size()));
    if (state.valid) {
        const double projectSampleRate = static_cast<double>(state.sampleRate);
        const uint32_t currentBlockSize = m_blockSize.load(std::memory_order_relaxed);
        // Every document-level invariant (track/region ids, references,
        // routing graph, sidechain cycles, plugin-array shape) is enforced by
        // native_project_load, which only returns a valid state when the file
        // is safe to hydrate.
        set_playing(false);
        waitForAudioCallbacks();
        // Build and validate the replacement document before mutating the
        // currently published project.  Hydration can fail on a malformed
        // sidechain, plugin index, missing allocation, or unsupported audio
        // state; none of those failures may erase the user's open project.
        std::unique_lock<std::mutex> lock(m_tracksMutex);
        std::vector<std::shared_ptr<Track>> loadedTracks;
        loadedTracks.reserve(state.tracks.size());
        // The plugin chain and curves are rebuilt by Rust once every provisional
        // track exists, so the loader hands it each track's own store handles.
        rust::Vec<::Hirari::Core::Bridge::NativeTrackStorages> pluginTargets;
        pluginTargets.reserve(state.tracks.size());
        // 1. Hydrate Tracks
        for (const auto& ts : state.tracks) {
            const auto trackType = static_cast<Track::Type>(ts.trackType);
            const std::string trackName = std::string(ts.pluginName);
            std::shared_ptr<Track> t;
            if (trackType == Track::Type::Bus) {
                auto busTrack = std::make_shared<BusTrack>(
                    ts.id, trackName.empty() ? "Bus" : trackName, ts.id,
                    m_projectLayoutRevision);
                // Do not mutate the process/session bus summing system while the
                // replacement document is still provisional. If hydration
                // later fails, the old project must remain the only visible
                // routing state. Resolve the bus after the commit point.
                t = std::move(busTrack);
            } else {
                t = std::make_shared<Track>(ts.id,
                    trackName.empty() ? "Track" : trackName, trackType,
                    m_projectLayoutRevision);
            }
            t->setVolume(ts.volume);
            t->setPan(ts.pan);
            t->setMuted(ts.muted);
            t->setSolo(ts.solo);
            t->setPhaseInverted(ts.phaseInvert);
            t->setRecordArmed(ts.recordArmed);
            t->setTrackDelaySamples(ts.trackDelaySamples);
            t->setSpatialPosition(ts.pan3d_x, ts.pan3d_y, ts.pan3d_z);
            pluginTargets.push_back(::Hirari::Core::Bridge::NativeTrackStorages{
                ts.id, ts.trackType,
                reinterpret_cast<size_t>(t->effectChainHandle()),
                reinterpret_cast<size_t>(t->automationStoreHandle())});
