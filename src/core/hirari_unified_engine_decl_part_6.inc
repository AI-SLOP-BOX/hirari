public:
    static constexpr uint32_t kMaxTracks = 256;
    static constexpr uint32_t kMaxTrackMap = 2048;

private:
    // Track identity plus the copy/free of the heap `std::shared_ptr<Track>`
    // handles the table owns. Defined with the rest of the table plumbing.
    static const HirariEngineTrackOps* trackOps() noexcept;
    void* m_trackTable = hirari_engine_track_table_create(trackOps());

    // Incremented around the complete direct callback. Control-side object
    // reclamation must not rely on transport state alone: a CoreAudio
    // callback can still be in flight while stop/reconnect is being handled.
    std::atomic<uint32_t> m_audioCallbacksInFlight{0};
    static constexpr uint32_t kNoInputMonitorTrack = std::numeric_limits<uint32_t>::max();
    // The monitored input pair, the talkback source and their publication
    // ordering live in Rust (engine_input_block.rs). The per-chunk read offset,
    // the playhead, the cycle, the loop flag and the sticky range-overflow
    // diagnostic are engine-block state (engine_block_host.rs).
    void* m_inputBlocks = hirari_input_blocks_create(kMaxAudioBlockSize);
    void* m_metronome = hirari_metronome_create(44100.0);
    std::atomic<float> m_masterGain{1.0f};
    std::atomic<double> m_sampleRate{44100.0};
    // Generation zero is reserved for an unavailable/uninitialized engine.
    // The constructor publishes the valid default 44.1 kHz configuration as
    // generation one before any callback or command can observe it.
    // The device configuration generation and the admission flag that closes
    // callbacks around a buffer replacement are transport state the Rust
    // engine block owns (engine_block_host.rs).

    // The command and event queues are Rust-owned SPSC rings (engine_rings.rs);
    // neither the records nor the storage for them live on the engine. The
    // handle is per engine so offline engines and tests never share a queue.
    void* m_engineRings = nullptr;

    // Internal components (using PIMPL principles where possible)
    ::Hirari::Core::AudioBuffer m_masterBuffer;
    // Callback-owned transport continuity (block end, snapshot revision,
    // playback pass, panic request) is Rust-owned; it reconciles held MIDI
    // notes after a seek, loop wrap, or transport restart without retriggering
    // notes on ordinary adjacent blocks. The block binds the handle, and so
    // owns the preview voices, the fallback note table and the block's own
    // event collection that used to live here.
    void* m_midiPlaybackClock = nullptr;
    // The native MIDI policy layer runs after timeline scheduling and before
    // preview/plugin dispatch, so articulation and MPE transformations affect
    // every downstream instrument consistently.
    HirariMpeState* m_mpeState = hirari_mpe_state_create();
    void* m_previewSynthState = hirari_preview_synth_create();

    // Preview-sample storage, trigger/playing flags and the playback cursor
    // live in Rust (preview_sampler.rs); the callback only renders the handle.
    void* m_previewSampler = hirari_preview_sampler_create();
    
