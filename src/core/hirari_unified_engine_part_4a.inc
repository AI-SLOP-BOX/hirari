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
        const uint32_t configuredBlockSize = m_context ? m_context->blockSize : 0u;
        // A freshly created bridge may not have an audio device configured
        // yet. Project hydration still needs prepared track buffers so that
        // routing and sidechain links can be rebound deterministically.
        const uint32_t currentBlockSize = configuredBlockSize == 0
            ? 512u
            : configuredBlockSize;
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
        // The bridge state is Rust-owned, so the few engine setters that take
        // standard containers materialize a control-side copy here.
        const auto toAutomation = [](const rust::Vec<::Hirari::Core::Bridge::NativeAutomationPoint>& points) {
            std::vector<ProjectAutomationPoint> converted;
            converted.reserve(points.size());
            for (const auto& point : points) {
                converted.push_back({point.time, point.value, point.curve});
            }
            return converted;
        };
        const auto toBlob = [](const rust::Vec<uint8_t>& bytes) {
            return std::vector<uint8_t>(bytes.begin(), bytes.end());
        };
        const auto toPluginAutomation =
            [&toAutomation](const rust::Vec<::Hirari::Core::Bridge::NativePluginAutomationLane>& lanes) {
                std::vector<ProjectPluginAutomationLane> converted;
                converted.reserve(lanes.size());
                for (const auto& lane : lanes) {
                    ProjectPluginAutomationLane value;
                    value.pluginIndex = lane.pluginIndex;
                    value.parameterId = lane.parameterId;
                    value.points = toAutomation(lane.points);
                    converted.push_back(std::move(value));
                }
                return converted;
            };
        // 1. Hydrate Tracks
        for (const auto& ts : state.tracks) {
            const auto trackType = static_cast<Track::Type>(ts.trackType);
            const std::string trackName = std::string(ts.pluginName);
            std::shared_ptr<Track> t;
            if (trackType == Track::Type::Bus) {
                auto busTrack = std::make_shared<BusTrack>(
                    ts.id, trackName.empty() ? "Bus" : trackName, ts.id,
                    m_sidechainEngine, m_projectLayoutRevision);
                // Do not mutate the process/session bus summing system while the
                // replacement document is still provisional. If hydration
                // later fails, the old project must remain the only visible
                // routing state. Resolve the bus after the commit point.
                t = std::move(busTrack);
            } else {
                t = std::make_shared<Track>(ts.id,
                    trackName.empty() ? "Track" : trackName, trackType,
                    m_sidechainEngine, m_projectLayoutRevision);
            }
            // Plugin arrays are positional within a serialized track. The
            // codec rejected inconsistent counts before this state was marked
            // valid, so the chain can be rebuilt positionally here.
            t->setVolume(ts.volume);
            t->setPan(ts.pan);
            t->setMuted(ts.muted);
            t->setSolo(ts.solo);
            t->setPhaseInverted(ts.phaseInvert);
            t->setRecordArmed(ts.recordArmed);
            t->setTrackDelaySamples(ts.trackDelaySamples);
            t->setSpatialPosition(ts.pan3d_x, ts.pan3d_y, ts.pan3d_z);
            if (!ts.volumeAutomation.empty()) t->setVolumeAutomation(toAutomation(ts.volumeAutomation));
            if (!ts.panAutomation.empty()) t->setPanAutomation(toAutomation(ts.panAutomation));
            if (!ts.trackDelayAutomation.empty()) t->setTrackDelayAutomation(toAutomation(ts.trackDelayAutomation));
            // pluginTypes is the container codec's decoded view of
            // pluginData: 0xffffffff marks a sandbox slot. Build the
            // processor chain in the saved order; restoring states before
            // creating sandbox processors makes a valid project fail when the
            // first state targets that processor.
            size_t sandboxPathIndex = 0;
            for (const uint32_t pluginType : ts.pluginTypes) {
                if (pluginType == 0xFFFFFFFFu) {
                    // The codec guarantees one sandbox path per marker. An
                    // unavailable binary is quarantined; the path is still
                    // consumed so the following plugin keeps its position.
                    (void)t->addSandboxedPlugin(std::string(ts.sandboxedPluginPaths[sandboxPathIndex]));
                    ++sandboxPathIndex;
                } else if (!t->addPlugin(pluginType)) {
                    return false;
                }
            }
            // Older files may have sandbox paths without the marker in
            // pluginData. Preserve compatibility by appending those entries.
            while (sandboxPathIndex < ts.sandboxedPluginPaths.size()) {
                (void)t->addSandboxedPlugin(std::string(ts.sandboxedPluginPaths[sandboxPathIndex++]));
            }
            if (!ts.pluginAutomation.empty()) t->setPluginAutomation(toPluginAutomation(ts.pluginAutomation));
            for (size_t stateIndex = 0; stateIndex < ts.pluginStates.size(); ++stateIndex) {
