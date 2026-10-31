    // DSP State (moved to pointer to decouple header)
    std::unique_ptr<::Hirari::DSP::ProcessContext> m_context;
    ::Hirari::Core::Concurrency::AudioTaskStealingScheduler m_threadPool;
    std::unique_ptr<::Hirari::DSP::Analysis::MasterMeter> m_analyzer;
    std::unique_ptr<::Hirari::DSP::Effects::MasterLimiter> m_masterLimiter;

    TelemetryData m_telemetryBuffers[2];
    // Per-engine Rust telemetry is allocated on the control thread and reused
    // by the audio callback; it must not lazily initialize on first process.
    void* m_mixerTelemetry = hirari_mixer_telemetry_create();
    std::atomic<uint32_t> m_activeTelemetryIdx{0};
    // Even values are stable snapshots; audio publication brackets writes
    // with odd/even values so control-rate readers can retry instead of
    // observing a buffer while the next audio block is overwriting it.
    std::atomic<uint64_t> m_telemetrySequence{0};
    mutable std::atomic_flag m_telemetryCopyLock = ATOMIC_FLAG_INIT;
    
    std::atomic<bool> m_isShutdown{false};
    // Project hydration mutates tracks, routing and several process-wide
    // managers as one control-plane transaction.  Reject a concurrent load
    // instead of allowing two documents to interleave partial state.
    std::atomic<bool> m_projectHydrationActive{false};
    std::atomic<bool> m_offlineRenderActive{false};
    std::atomic<uint64_t> m_offlineRenderOwner{0};
    std::atomic<bool> m_offlineTargetActive{false};
    std::atomic<uint32_t> m_offlineTargetId{0};
    std::atomic<uint32_t> m_offlineRenderTailMillis{0};
    std::atomic<bool> m_offlineRenderPreFader{false};
    std::atomic<bool> m_offlineRenderIncludeInserts{true};
    std::atomic<bool> m_offlineRenderRangeActive{false};
    std::atomic<uint64_t> m_offlineRenderStartSample{0};
    std::atomic<uint64_t> m_offlineRenderEndSample{0};
    std::atomic<bool> m_bounceRunning{false};
    std::atomic<float> m_bounceProgress{0.0f};
    std::atomic<uint32_t> m_bounceState{static_cast<uint32_t>(BounceState::Idle)};
    std::atomic<bool> m_bounceCancelRequested{false};
    std::atomic<bool> m_bouncePauseRequested{false};
    // Serializes cancellation with the final temporary-file publish so a
    // cancel cannot race a stale render into the destination path.
    mutable std::mutex m_bouncePublishMutex;
    // Serializes render thread retirement with a subsequent start. The
    // worker publishes Failed/Completed before its wrapper flips
    // m_bounceRunning, so callers must be able to join a terminal worker
    // before accepting the next render.
    mutable std::mutex m_bounceLifecycleMutex;
    std::thread m_bounceThread;
    std::shared_ptr<ProjectLayoutRevision> m_projectLayoutRevision =
        std::make_shared<ProjectLayoutRevision>();
    std::atomic<uint64_t> m_projectControlRevision{0};
    std::vector<std::shared_ptr<Track>> m_tracks;
    std::vector<std::shared_ptr<Track>> m_deferredDestructionTracks;
    mutable std::mutex m_tracksMutex;

    // Id resolution shared by the region, routing, control and plugin host
    // callbacks. Callers must already hold m_tracksMutex.
    Track* findTrackLocked(uint32_t tid);
    const Track* findTrackLocked(uint32_t tid) const;
    std::shared_ptr<Track> findTrackSharedLocked(uint32_t tid);
    std::shared_ptr<Track> findTrackSharedLocked(uint32_t tid) const;
    const Region* findRegionLocked(uint32_t tid, uint32_t rid) const;

    // Sidechain route bookkeeping is project state owned by Rust
    // (sidechain_route_store.rs); the engine keeps only the opaque handle.
    void* m_sidechainRoutes = hirari_sidechain_routes_create();

    // Borrowed layout snapshot shared by the JSON writer and the project saver.
    // Serialization borrows data owned by each track, so every per-track block
    // is heap-allocated: moving it would invalidate the pointers to
    // short-string buffers the snapshot holds.
    struct ProjectLayoutStorage {
        HirariLayoutTrack view{};
        std::string name;
        std::string frozenPath;
        std::string expressionMap;
        std::string expressionMapPro;
        std::vector<HirariLayoutAutomationPoint> volumeAutomation;
        std::vector<HirariLayoutAutomationPoint> panAutomation;
        std::vector<HirariLayoutAutomationPoint> trackDelayAutomation;
        std::vector<HirariLayoutPluginLane> pluginAutomation;
        std::vector<std::vector<Hirari::Core::ProjectAutomationPoint>> pluginAutomationPoints;
        std::vector<uint8_t> pluginBypass;
        std::vector<std::vector<float>> pluginParameters;
        std::vector<HirariLayoutParameterBlock> pluginParameterBlocks;
        std::vector<std::vector<uint8_t>> pluginStates;
        std::vector<HirariLayoutByteBlock> pluginStateBlocks;
        std::vector<std::vector<uint8_t>> pluginGuiStates;
        std::vector<HirariLayoutByteBlock> pluginGuiStateBlocks;
        std::vector<HirariLayoutString> sandboxPaths;
        std::vector<std::vector<uint8_t>> sandboxStates;
        std::vector<HirariLayoutByteBlock> sandboxStateBlocks;
        std::vector<HirariLayoutRegion> regions;
    };
    struct ProjectLayoutData {
        std::vector<std::unique_ptr<ProjectLayoutStorage>> storages;
        std::vector<HirariLayoutTrack> trackViews;
        std::vector<HirariLayoutSidechainRoute> sidechainRoutes;
        std::vector<HirariLayoutFeedbackRoute> feedbackRoutes;
        HirariProjectLayoutSnapshot snapshot{};
    };
    // `includeSaveOnlyFields` additionally collects the plug-in GUI blobs, which
    // the polling JSON path must not copy.
    bool collectProjectLayout(ProjectLayoutData* out, bool includeSaveOnlyFields,
                              const std::atomic<bool>* cancellation) const;

    void* m_midiSnapshotStore = nullptr;

    std::atomic<float> m_dspLoad{0.0f};
    std::atomic<uint32_t> m_diagnosticBlockCounter{0};
    // Zero is reserved as an invalid/unset region identity in serialized
    // projects and FFI calls. All runtime-created regions start at one.
    std::atomic<uint32_t> m_regionIdCounter{1};
    std::atomic<uint32_t> m_trackIdCounter{0};
    std::atomic<bool> m_showVideo{false};
    std::atomic<bool> m_isAscended{false};
    std::atomic<uint32_t> m_autoRecordMode{0};
    std::atomic<int32_t> m_rootNote{0}, m_scaleType{0};
    std::atomic<uint32_t> m_articulationMapHash{0};
