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
    Hirari::Core::EffectChain m_effectChain;
    MidiBuffer m_pluginMidi;
    void* m_articulationState = nullptr;
    void* m_builtInMidiState = nullptr;
    double m_sampleRate = 44100.0;
    std::atomic<double> m_transportBpm{120.0};
    std::atomic<bool> m_transportPlaying{false};
    uint32_t m_blockSize = 0;
    bool m_isPrepared = false;
    std::atomic<float> m_volume{1.0f};
    std::atomic<float> m_pan{0.0f};
    std::atomic<bool> m_muted{false};
    std::atomic<bool> m_phaseInvert{false};
    std::atomic<bool> m_solo{false};
    std::atomic<bool> m_recordArmed{false};
    // The Rust region store owns the table and publishes immutable render
    // inputs; the audio callback reads only these three atomics.
    std::atomic<const HirariTrackRegionRenderInput*> m_regionInputs{nullptr};
    std::atomic<size_t> m_regionInputCount{0};
    std::atomic<uint8_t> m_regionPublished{0};
    std::atomic<bool> m_audioPitchAnalysisRunning{false};
    // Updated on the control thread from sandbox-reported plugin metadata;
    // the callback reads it to choose the built-in MIDI fallback.
    std::atomic<uint32_t> m_midiInstrumentPluginCount{0};
    // Rust-owned slot registry: chain index and path per sandboxed plugin.
    void* m_sandboxSlots = hirari_sandbox_slots_create();
    std::atomic<uint8_t> m_lastSandboxFailure{0};
    std::string m_lastSandboxFailureText = "none";
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
    float m_lowGain = 1.0f;
    float m_highGain = 1.0f;
    float m_lowStateL = 0.0f;
    float m_lowStateR = 0.0f;
    void* m_automationStore = hirari_track_automation_store_create();
    std::atomic<float> m_spatialX{0.0f}, m_spatialY{0.0f}, m_spatialZ{0.0f};
    // The spatial processor is Rust-owned; the track keeps only its handle.
    void* m_spatialPanner = hirari_track_holographic_panner_create();
    static constexpr uint32_t kMaxHrtfTaps = 128;
    static constexpr uint32_t kMaxPdcDelaySamples = 8192;
    std::atomic<uint32_t> m_pdcRequested{0};
    std::atomic<uint32_t> m_trackDelaySamples{0};
    void* m_pdcDelayState = nullptr;
};
