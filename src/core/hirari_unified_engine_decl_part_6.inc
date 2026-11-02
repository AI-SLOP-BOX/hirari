public:
    static constexpr uint32_t kMaxTracks = 256;
    static constexpr uint32_t kMaxTrackMap = 2048;

private:
    // Audio engine state management (Linus-Grade: Cache-aligned cluster)
    struct alignas(64) AudioThreadState {
        Track* activeTracks[kMaxTracks];
        Track* trackMap[kMaxTrackMap]; // INDUSTRIAL: O(1) Track Lookup by ID
        uint32_t trackCount = 0;
        uint64_t audioConfigGeneration = 0;
    };

    AudioThreadState m_audioStates[2];
    std::atomic<uint32_t> m_activeStateIdx{0};

    std::atomic<bool> m_isPlaying{false};
    // Incremented around the complete direct callback. Control-side object
    // reclamation must not rely on transport state alone: a CoreAudio
    // callback can still be in flight while stop/reconnect is being handled.
    std::atomic<uint32_t> m_audioCallbacksInFlight{0};
    std::atomic<bool> m_testTone{false};
    static constexpr uint32_t kNoInputMonitorTrack = std::numeric_limits<uint32_t>::max();
    std::atomic<uint32_t> m_inputMonitorTrackId{kNoInputMonitorTrack};
    std::atomic<uint64_t> m_inputMonitorChannelPair{(uint64_t{1} << 32)};
    std::array<float, kMaxAudioBlockSize> m_inputMonitorLeft{};
    std::array<float, kMaxAudioBlockSize> m_inputMonitorRight{};
    std::atomic<uint32_t> m_inputMonitorBlockFrames{0};
    std::atomic<bool> m_inputMonitorBlockPublished{false};
    uint32_t m_inputMonitorBlockOffset = 0;
    std::array<float, kMaxAudioBlockSize> m_talkbackInput{};
    std::array<float, kMaxAudioBlockSize> m_metronomeClickLeft{};
    std::array<float, kMaxAudioBlockSize> m_metronomeClickRight{};
    std::atomic<uint32_t> m_talkbackInputFrames{0};
    std::atomic<bool> m_talkbackInputPublished{false};
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
    
