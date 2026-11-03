public:
    static constexpr uint32_t kMaxTracks = 256;
    static constexpr uint32_t kMaxTrackMap = 2048;

private:
    // The realtime window onto the Rust-owned track table (engine_track_table.rs).
    // Publication fills the buffer the callback is not reading and only then
    // flips the index, so these pointers are traversed without a lock.
    struct TrackView {
        Track* const* activeTracks;  // O(1) by slot, in graph order
        Track* const* trackMap;      // O(1) Track lookup by id
        uint32_t trackCount;
        uint64_t audioConfigGeneration;
    };

    static TrackView trackView(void* table) noexcept {
        HirariTrackSnapshot published{};
        hirari_engine_track_snapshot(table, &published);
        return { reinterpret_cast<Track* const*>(published.active_tracks),
                 reinterpret_cast<Track* const*>(published.track_map),
                 published.track_count, published.audio_config_generation };
    }

    // Track identity plus the copy/free of the heap `std::shared_ptr<Track>`
    // handles the table owns. Defined with the rest of the table plumbing.
    static const HirariEngineTrackOps* trackOps() noexcept;
    void* m_trackTable = hirari_engine_track_table_create(trackOps());

    std::atomic<bool> m_isPlaying{false};
    // Incremented around the complete direct callback. Control-side object
    // reclamation must not rely on transport state alone: a CoreAudio
    // callback can still be in flight while stop/reconnect is being handled.
    std::atomic<uint32_t> m_audioCallbacksInFlight{0};
    std::atomic<bool> m_testTone{false};
    static constexpr uint32_t kNoInputMonitorTrack = std::numeric_limits<uint32_t>::max();
    // The monitored input pair, the talkback source and their publication
    // ordering live in Rust (engine_input_block.rs); only the per-chunk read
    // offsets are callback-local state the engine itself decides.
    void* m_inputBlocks = hirari_input_blocks_create(kMaxAudioBlockSize);
    uint32_t m_inputMonitorBlockOffset = 0;
    std::array<float, kMaxAudioBlockSize> m_metronomeClickLeft{};
    std::array<float, kMaxAudioBlockSize> m_metronomeClickRight{};
    uint32_t m_talkbackInputOffset = 0;
    std::atomic<uint64_t> m_playhead{0};
    // Sticky diagnostic: a saturated sample position or invalid callback
    // range was observed. It is intentionally lock-free so the control/UI
    // side can report it without touching the audio callback.
    std::atomic<bool> m_audioRangeOverflow{false};
    std::atomic<bool> m_loopEnabled{false};
    std::atomic<uint64_t> m_cycleStart{0};
    std::atomic<uint64_t> m_cycleEnd{0};
    void* m_metronome = hirari_metronome_create(44100.0);
    std::atomic<float> m_masterGain{1.0f};
    std::atomic<double> m_sampleRate{44100.0};
    // Generation zero is reserved for an unavailable/uninitialized engine.
    // The constructor publishes the valid default 44.1 kHz configuration as
    // generation one before any callback or command can observe it.
    std::atomic<uint64_t> m_audioConfigGeneration{1};
    std::atomic<bool> m_audioConfigTransition{false};

    // The command and event queues are Rust-owned SPSC rings (engine_rings.rs);
    // neither the records nor the storage for them live on the engine. The
    // handle is per engine so offline engines and tests never share a queue.
    void* m_engineRings = nullptr;

    // Internal components (using PIMPL principles where possible)
    ::Hirari::Core::AudioBuffer m_masterBuffer;
    ::Hirari::Core::MidiBuffer m_blockMidi;
    // Callback-owned transport continuity (block end, snapshot revision,
    // playback pass, panic request) is Rust-owned; it reconciles held MIDI
    // notes after a seek, loop wrap, or transport restart without retriggering
    // notes on ordinary adjacent blocks.
    void* m_midiPlaybackClock = nullptr;
    // The native MIDI policy layer runs after timeline scheduling and before
    // preview/plugin dispatch, so articulation and MPE transformations affect
    // every downstream instrument consistently.
    HirariMpeState* m_mpeState = hirari_mpe_state_create();
    void* m_previewSynthState = hirari_preview_synth_create();
    // Audio-thread ownership. This preserves the preview routing decision for
    // a held note even if a plug-in is inserted or removed before Note Off.
    uint32_t m_previewFallbackActiveNotes[16][128]{};
    std::atomic<bool> m_previewVoicesNeedReset{false};

    // Preview-sample storage, trigger/playing flags and the playback cursor
    // live in Rust (preview_sampler.rs); the callback only renders the handle.
    void* m_previewSampler = hirari_preview_sampler_create();
    
