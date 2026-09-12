    // DSP State (moved to pointer to decouple header)
    std::unique_ptr<::Aura::DSP::ProcessContext> m_context;
    ::Aura::Core::Concurrency::AudioTaskStealingScheduler m_threadPool;
    std::unique_ptr<::Aura::DSP::Analysis::MasterMeter> m_analyzer;
    std::unique_ptr<::Aura::DSP::Effects::MasterLimiter> m_masterLimiter;
    
    // Aesthetic Sovereignty
    // INDUSTRIAL: Lock-Free Atomic State (Trivially Copyable)
    std::atomic<Mixing::AestheticFeatures> m_vibeData{ Mixing::AestheticFeatures{} };
    std::atomic<uint32_t> m_vibeVersion{0}; 

    TelemetryData m_telemetryBuffers[2];
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
    std::vector<std::shared_ptr<Track>> m_tracks;
    std::vector<std::shared_ptr<Track>> m_deferredDestructionTracks;
    mutable std::mutex m_tracksMutex;

    struct SidechainRoute {
        uint32_t sourceId = 0;
        uint32_t destinationId = 0;
        uint32_t pluginIndex = 0;
        uint32_t tapPoint = 2;
    };
    std::vector<SidechainRoute> m_sidechainRoutes;

    // UI/control thread publishes a complete immutable snapshot; the audio
    // thread only reads the active slot and never waits for a mutex.
    std::vector<ScheduledMidiNote> m_midiNoteSnapshots[2];
    std::atomic<uint32_t> m_activeMidiSnapshot{0};
    mutable std::mutex m_midiNotesMutex;

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
