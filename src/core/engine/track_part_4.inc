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
    Type m_type;
    // The Rust channel strip owns this track's fader, pan, solo, record arm and
    // sample rate; the track keeps only the handle it reads them through.
    void* m_strip = hirari_channel_strip_create();
    // Rust owns every insert: node instances, order, sandbox identity and the
    // built-in-instrument count. The track keeps only the chain handle.
    void* m_chain = hirari_effect_chain_create();
    // The non-finite poll cache moved into the Rust chain host, which keeps the
    // last measurement outside its control mutex.
    // Rust owns the block's event table; the head stages into it and the chain
    // reads it in place, so the track keeps only the handle.
    void* m_pluginMidi = hirari_midi_buffer_create();
    void* m_articulationState = nullptr;
    void* m_builtInMidiState = nullptr;
    // The block pipeline and its taps are Rust-owned storages; the track only
    // holds their handles and their lifetime.
    void* m_activeBuffer = hirari_audio_buffer_storage_create();
    void* m_preFaderBuffer = hirari_audio_buffer_storage_create();
    void* m_sendPreFaderBuffer = hirari_audio_buffer_storage_create();
    void* m_sendPostFaderBuffer = hirari_audio_buffer_storage_create();
    void* m_preInsertBuffer = hirari_audio_buffer_storage_create();
    // Control-thread-owned retention prevents an in-flight audio callback
    // from observing freed freeze memory; the store publishes the read-side
    // pointer through an atomic the callback loads without locking, and owns
    // the rendered buffers it holds.
    void* m_frozenStore = hirari_track_frozen_store_create();
    void* m_automationStore = hirari_track_automation_store_create();
    // The name is Rust state too, so the layout gather and the control edit host
    // read and write it through this handle like every other track storage.
    void* m_nameStore = hirari_track_name_store_create();
    // The spatial processor is Rust-owned; the track keeps only its handle.
    void* m_spatialPanner = hirari_track_holographic_panner_create();
    static constexpr uint32_t kMaxHrtfTaps = 128;
    void* m_pdcDelayState = nullptr;
    // Every value the audio callback reads, plus the block pipeline itself, is
    // bound into this runtime; it is created last and destroyed first.
    void* m_runtime = nullptr;
};
