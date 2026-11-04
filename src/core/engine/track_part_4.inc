    // Region layout is shared with the Rust store, so the ABI it publishes
    // through has to stay pinned here too.
    static_assert(sizeof(HirariWarpMarker) == 24);
    static_assert(offsetof(HirariWarpMarker, sourceSample) == 0);
    static_assert(offsetof(HirariWarpMarker, timelineSample) == 8);
    static_assert(offsetof(HirariWarpMarker, transient) == 16);
    static_assert(sizeof(HirariRegionBlockConfig) == 88);
    static_assert(sizeof(HirariRegionBlockOutput) == 56);
    static_assert(sizeof(HirariRegionRenderConfig) == 128);
    static_assert(sizeof(HirariAudioNoteCurveView) == 64);
    static_assert(offsetof(HirariAudioNoteCurveView, formantOffsetCents) == 56);
    static_assert(sizeof(HirariTrackRegionRenderInput) == 312);
    static_assert(offsetof(HirariTrackRegionRenderInput, playhead) == 160);
    static_assert(offsetof(HirariTrackRegionRenderInput, warpRatio) == 248);
    static_assert(offsetof(HirariTrackRegionRenderInput, muted) == 300);

    std::shared_ptr<ProjectLayoutRevision> m_layoutRevision;
    uint32_t m_id;
    std::string m_name;
    Type m_type;
    // Channel-strip gain/pan/solo state is Rust-owned; this is its handle.
    void* m_strip = hirari_channel_strip_create();
    // Rust owns every insert: node instances, order, sandbox identity and the
    // built-in-instrument count. The track keeps only the chain handle.
    void* m_chain = hirari_effect_chain_create();
    // Last sampled non-finite count, so a polled diagnostic never blocks.
    mutable std::atomic<uint64_t> m_lastNonFiniteSamples{0};
    // Rust owns the block's event table; the head stages into it and the chain
    // reads it in place, so the track keeps only the handle.
    void* m_pluginMidi = hirari_midi_buffer_create();
    void* m_articulationState = nullptr;
    void* m_builtInMidiState = nullptr;
    double m_sampleRate = 44100.0;
    std::atomic<float> m_volume{1.0f};
    std::atomic<float> m_pan{0.0f};
    std::atomic<bool> m_solo{false};
    std::atomic<bool> m_recordArmed{false};
    std::atomic<bool> m_audioPitchAnalysisRunning{false};
    AudioBuffer m_activeBuffer;
    AudioBuffer m_preFaderBuffer;
    AudioBuffer m_sendPreFaderBuffer;
    AudioBuffer m_sendPostFaderBuffer;
    AudioBuffer m_preInsertBuffer;
    // Control-thread-owned retention prevents an in-flight audio callback
    // from observing freed freeze memory; the store publishes the read-side
    // pointer through an atomic the callback loads without locking.
    static void destroyFrozenOwner(void* owner) {
        delete static_cast<std::shared_ptr<AudioBuffer>*>(owner);
    }
    static constexpr HirariTrackFrozenOps kFrozenOps = {&Track::destroyFrozenOwner};
    void* m_frozenStore = hirari_track_frozen_store_create(&kFrozenOps);
    void* m_automationStore = hirari_track_automation_store_create();
    // The spatial processor is Rust-owned; the track keeps only its handle.
    void* m_spatialPanner = hirari_track_holographic_panner_create();
    static constexpr uint32_t kMaxHrtfTaps = 128;
    void* m_pdcDelayState = nullptr;
    // Every value the audio callback reads, plus the block pipeline itself, is
    // bound into this runtime; it is created last and destroyed first.
    void* m_runtime = nullptr;
};
