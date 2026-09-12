    std::string m_pluginPath;
    std::string m_requestedFormat = "auto";
    double m_sampleRate = 44100.0;
    uint32_t m_maxBlockSize = 512;
    uint32_t m_channels = SandboxProtocol::kMaxChannels;
    std::atomic<bool> m_processAlive;
    std::atomic<uint32_t> m_processCalls{0};
    // Audio calls observe this gate without taking the state mutex.  The
    // control thread sets it before waiting for in-flight calls, closing the
    // race between the wait and the first state mailbox write.
    std::atomic<bool> m_stateTransition{false};
    std::atomic<Failure> m_failure{Failure::None};
    std::atomic<uint64_t> m_heartbeat{0};
#if !defined(_WIN32)
    std::atomic<pid_t> m_pid{-1};
    int m_controlFd = -1;
    int m_statusFd = -1;
    int m_sharedFd = -1;
    std::string m_sharedName;
    SandboxProtocol::SharedAudioBlock* m_shared = nullptr;
    inline static std::atomic<uint64_t> s_sharedNameCounter{0};
#endif
    uint64_t m_sequence = 0;
    mutable std::mutex m_stateMutex;
    // Lifecycle operations are serialized as one control-plane transaction.
    // recursive_mutex is intentional: restart/reconfigure call start/stop
    // internally while holding this lock. The audio callback never takes it.
    mutable std::recursive_mutex m_lifecycleMutex;
    mutable uint64_t m_stateSequence = 0;
    GenerationContext m_generationContext{};
    mutable std::atomic<uint8_t> m_lastStateError{SandboxProtocol::kStateErrorNone};
    std::vector<uint8_t> m_stateCache;
    uint32_t m_restartAttempts = 0;
    std::chrono::steady_clock::time_point m_nextRestartTime{};
    uint64_t m_lastSubmitted = 0;
    uint64_t m_pendingAgeFrames = 0;
    // Audio-thread-owned; it is reset whenever a new mailbox sequence is
    // submitted or the previous sequence is collected.
    bool m_overrunReportedForSequence = false;
    bool m_sequenceHadOverrun = false;
    bool m_completedSequenceHadOverrun = false;
    alignas(64) float m_pendingInput[SandboxProtocol::kMaxChannels][SandboxProtocol::kMaxFrames]{};
    SandboxProtocol::MidiEvent m_pendingMidi[SandboxProtocol::kMaxMidiEvents]{};
    std::array<SandboxProtocol::ParameterChange,
               SandboxProtocol::kMaxParameterChanges> m_pendingParameterChanges{};
    std::atomic<uint64_t> m_parameterWrite{0};
    std::atomic<uint64_t> m_parameterRead{0};
    uint32_t m_pendingChannels = 0;
    uint32_t m_pendingFrames = 0;
    std::atomic<bool> m_lastProcessOverrun{false};
    uint64_t m_lastHeartbeat = 0;
    uint64_t m_lastCompletedSequence = 0;
#if !defined(_WIN32)
    std::chrono::steady_clock::time_point m_lastHeartbeatTime{};
    std::chrono::steady_clock::time_point m_lastProgressTime{};
#endif
