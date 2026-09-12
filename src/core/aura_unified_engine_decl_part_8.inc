    // SBF-v6 Workspace Sovereignty
    struct Workspace {
        std::atomic<float> zoomX{1.0f}, zoomY{1.0f};
        std::atomic<uint64_t> scrollPos{0};
        std::atomic<uint32_t> focusedTrackId{0};
    } m_workspace;


    // Phase 9: Forensic Peak Cache (Zero-Wait UI)
    struct WaveformCache {
        float* mappedData = nullptr;
        size_t mappedSize = 0;
        int fd = -1;
        std::atomic<bool> valid{false};
        uint32_t sampleCount = 0;
        std::string filepath;

        ~WaveformCache();
        
        WaveformCache() = default;
        WaveformCache(const WaveformCache&) = delete;
        WaveformCache& operator=(const WaveformCache&) = delete;
        
        WaveformCache(WaveformCache&& other) noexcept {
            mappedData = other.mappedData;
            mappedSize = other.mappedSize;
            fd = other.fd;
            valid.store(other.valid.load());
            sampleCount = other.sampleCount;
            filepath = std::move(other.filepath);
            
            other.mappedData = nullptr;
            other.mappedSize = 0;
            other.fd = -1;
        }
        
        WaveformCache& operator=(WaveformCache&& other) noexcept {
            if (this != &other) {
                cleanup();
                mappedData = other.mappedData;
                mappedSize = other.mappedSize;
                fd = other.fd;
                valid.store(other.valid.load());
                sampleCount = other.sampleCount;
                filepath = std::move(other.filepath);
                
                other.mappedData = nullptr;
                other.mappedSize = 0;
                other.fd = -1;
            }
            return *this;
        }

    public:
        void cleanup();
    };
    std::unordered_map<uint32_t, WaveformCache> m_peakCache;
    // A region can be rebuilt while an older project generation is still
    // draining. Store the generation so an old worker cannot erase a newer
    // build registration.
    std::unordered_map<uint32_t, uint64_t> m_peakBuilds;
    std::atomic<uint64_t> m_waveformGeneration{0};
    mutable std::mutex m_cacheMutex;

    std::unique_ptr<ProcessGraph> m_processGraph;
    // PDC is project/session state. It must not be shared through the legacy
    // process-wide compatibility singleton.
    PDCManager m_pdcManager;
    TonalSync m_tonalSync;
    UndoTransactionManager m_undoManager;
    TempoMap m_tempoMap;
    RoutingEngine m_routingEngine;
    SidechainManager m_sidechainManager;
    BusSystem m_busSystem;
    MacroControlManager m_macroControlManager;
    ::Aura::Core::MidiLearnManager m_midiLearnManager;
    ::Aura::Core::Diagnostics::EngineDiagnostics m_engineDiagnostics;
    ::Aura::Core::Diagnostics::ForensicJournaler m_forensicJournaler;
    ::Aura::Core::IO::AudioDecoderManager m_audioDecoderManager;
    std::atomic<bool> m_graphDirty{true};

    // Engine Infrastructure
    std::unique_ptr<ScriptManager> m_scriptManager;
    
    // RT-Safe Pre-allocated Kernels
    ::Aura::Core::DSP::Effects::AtmosphericProcessorKernel m_atmosphericEngine;
    ::Aura::Core::Mixing::MasteringKernel m_masteringKernel;
    ::Aura::Core::Mixing::ControlRoom m_controlRoom;
    ::Aura::Core::Mixing::TrackPresetStore m_trackPresets;
    ::Aura::Core::Plugins::PluginCompatibilityRegistry m_pluginCompatibility;
    ::Aura::Core::Engine::AutomationLaneGroup m_automationLanes;
    ::Aura::Core::Engine::TapTempo m_tapTempo;
    ::Aura::Core::MIDI::ExternalSync m_externalSync;
    ::Aura::Core::Composition::ArrangementEditModel m_arrangement;
    mutable std::atomic<bool> m_ecoModeLogTriggered{false};
    mutable std::atomic<bool> m_videoSyncLogTriggered{false};

    // Phase 10: Sovereign Recovery
    void startRecoveryOrchestrator();
    std::thread m_recoveryThread;
    std::atomic<bool> m_recoveryRunning{false};
    std::condition_variable m_recoveryWake;
    std::mutex m_recoveryWakeMutex;
    // Manual health polling and the background recovery loop share the same
    // sandbox host objects. Serialize maintenance so stop/restart cannot race
    // with another poll on the same worker or IPC mapping.
    mutable std::mutex m_sandboxMaintenanceMutex;
    struct SandboxObservedStatus {
        bool alive = false;
        uint8_t failure = 0;
        uint32_t droppedOutputMidi = 0;
    };
    std::unordered_map<uint64_t, SandboxObservedStatus> m_sandboxObserved;
};
