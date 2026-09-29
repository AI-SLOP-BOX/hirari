public:
    static constexpr uint32_t kMaxTracks = 256;
    static constexpr uint32_t kMaxTrackMap = 2048;

private:
    bool write_project_layout_json(std::ostream& output,
                                   const std::atomic<bool>* cancellation = nullptr) const;
    uint32_t allocateTrackIdLocked() noexcept;
    uint32_t allocateRegionIdLocked() noexcept;
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
    ::Hirari::Core::Engine::Metronome m_metronome;
    std::atomic<float> m_masterGain{1.0f};
    std::atomic<double> m_sampleRate{44100.0};
    // Generation zero is reserved for an unavailable/uninitialized engine.
    // The constructor publishes the valid default 44.1 kHz configuration as
    // generation one before any callback or command can observe it.
    std::atomic<uint64_t> m_audioConfigGeneration{1};
    std::atomic<bool> m_audioConfigTransition{false};
    
    // Lock-free queues
    struct Command {
        ::Hirari::Core::CommandType type;
        uint32_t tid;
        float val;
        uint64_t ts;
        // Commands are valid only for the audio configuration snapshot from
        // which the UI/CLI observed them.  The callback rechecks this before
        // mutating a Track, preventing a queued command from crossing a
        // sample-rate or block-size transition.
        uint64_t expectedAudioGeneration{0};
    };

    enum class CommandPushError : uint8_t {
        None = 0,
        InvalidTarget = 1,
        StaleProjectGeneration = 2,
        StaleAudioGeneration = 3,
        QueueFull = 4,
    };

    std::atomic<uint8_t> m_lastCommandError{
        static_cast<uint8_t>(CommandPushError::None)};

    ::Hirari::Core::RingBuffer<Command, 1024> m_commandQueue;
    mutable ::Hirari::Core::RingBuffer<::Hirari::Core::EngineEvent, 512> m_eventQueue;

    // Internal components (using PIMPL principles where possible)
    ::Hirari::Core::AudioBuffer m_masterBuffer;
    ::Hirari::Core::MidiBuffer m_blockMidi;
    // Callback-owned transport continuity used to reconcile held MIDI notes
    // after a seek, loop wrap, or transport restart without retriggering notes
    // on ordinary adjacent blocks.
    uint64_t m_previousMidiBlockEnd = 0;
    uint64_t m_previousMidiSnapshotRevision = 0;
    uint64_t m_midiPlaybackPass = 0;
    bool m_hasPreviousMidiBlock = false;
    bool m_previousMidiPlaybackActive = false;
    std::atomic<bool> m_midiPanicRequested{false};
    // The native MIDI policy layer runs after timeline scheduling and before
    // preview/plugin dispatch, so articulation and MPE transformations affect
    // every downstream instrument consistently.
    ::Hirari::Core::Engine::MIDIOrchestrator m_midiOrchestrator;
    PreviewSynth m_previewSynth;
    // Audio-thread ownership. This preserves the preview routing decision for
    // a held note even if a plug-in is inserted or removed before Note Off.
    uint32_t m_previewFallbackActiveNotes[16][128]{};
    std::atomic<bool> m_previewVoicesNeedReset{false};

    struct PreviewSample {
        std::vector<float> samples;
        double sourceRate = 44100.0;
    };
    // Append-only generations are owned by the control side. The audio
    // callback reads only this pointer and never touches a shared_ptr refcount.
    std::vector<std::unique_ptr<const PreviewSample>> m_previewSampleStorage;
    std::atomic<const PreviewSample*> m_previewSample{nullptr};
    std::atomic<bool> m_previewSampleTrigger{false};
    // A loaded preview asset is not the same thing as an actively playing
    // preview. Keeping this state separate lets a stopped project take the
    // cheap silence path instead of traversing the entire audio graph.
    std::atomic<bool> m_previewSamplePlaying{false};
    double m_previewSamplePosition = 0.0;
    
