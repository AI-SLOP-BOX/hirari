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
    void* track = findPublishedTrack(tid);
    return track != nullptr && hirari_region_store_append_processing(track, rid, step);
}
bool HirariUnifiedEngine::clear_region_processing(uint32_t tid, uint32_t rid) {
    void* track = findPublishedTrack(tid);
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
        // A provisional track is a Rust object plus the owning handle that keeps
        // it alive until the graph takes the handle over at the commit point. If
        // hydration fails, releasing the handle tears the track down again.
        struct Provisional {
            void* owner = nullptr;
            void* track = nullptr;
            Provisional(void* ownerHandle, void* object) : owner(ownerHandle), track(object) {}
            Provisional(Provisional&& other) noexcept
                : owner(other.owner), track(other.track) { other.owner = nullptr; }
            Provisional(const Provisional&) = delete;
            ~Provisional() {
                if (owner) hirari_track_owner_release(owner);
            }
            // Gives the graph the handle and stops this entry from releasing it.
            void* commit() {
                void* taken = owner;
                owner = nullptr;
                return taken;
            }
        };
        std::vector<Provisional> loadedTracks;
        loadedTracks.reserve(state.tracks.size());
        // The plugin chain and curves are rebuilt by Rust once every provisional
        // track exists, so the loader hands it each track's own store handles.
        rust::Vec<::Hirari::Core::Bridge::NativeTrackStorages> pluginTargets;
        pluginTargets.reserve(state.tracks.size());
        // 1. Hydrate Tracks
        for (const auto& ts : state.tracks) {
            const std::string trackName = std::string(ts.pluginName);
            const std::string named = trackName.empty()
                ? (ts.trackType == kHirariTrackKindBus ? "Bus" : "Track")
                : trackName;
            void* track = nullptr;
            void* owner = hirari_track_new(
                ts.id, reinterpret_cast<const uint8_t*>(named.data()), named.size(),
                ts.trackType, m_layoutRevision, &track);
            if (!owner) return false;
            // A bus keeps its slot only once the bus system resolves it after the
            // commit point; while provisional it must not touch the summing system.
            loadedTracks.emplace_back(owner, track);
            hirari_track_set_volume(track, ts.volume);
            hirari_track_set_pan(track, ts.pan);
            hirari_track_set_muted(track, ts.muted);
            hirari_track_set_solo(track, ts.solo);
            hirari_track_set_phase_inverted(track, ts.phaseInvert);
            (void)hirari_track_set_record_armed(track, ts.recordArmed);
            hirari_track_set_delay_samples(track, ts.trackDelaySamples);
            hirari_track_set_spatial(track, ts.pan3d_x, ts.pan3d_y, ts.pan3d_z);
            pluginTargets.push_back(::Hirari::Core::Bridge::NativeTrackStorages{
                ts.id, ts.trackType,
                reinterpret_cast<size_t>(
                    hirari_track_storage(track, kHirariTrackStoreChain)),
                reinterpret_cast<size_t>(
                    hirari_track_storage(track, kHirariTrackStoreAutomation))});
        }
