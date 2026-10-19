    // SBF-v6 Workspace Sovereignty
    struct Workspace {
        std::atomic<float> zoomX{1.0f}, zoomY{1.0f};
        std::atomic<uint64_t> scrollPos{0};
        std::atomic<uint32_t> focusedTrackId{0};
    } m_workspace;


    // Phase 9: Forensic Peak Cache (Zero-Wait UI)
    struct WaveformCache {
        void* rustMapping = nullptr;
        std::atomic<bool> valid{false};

        ~WaveformCache();

        WaveformCache() = default;
        WaveformCache(const WaveformCache&) = delete;
        WaveformCache& operator=(const WaveformCache&) = delete;

        WaveformCache(WaveformCache&& other) noexcept {
            rustMapping = other.rustMapping;
            valid.store(other.valid.load());
            other.rustMapping = nullptr;
            other.valid.store(false);
        }

        WaveformCache& operator=(WaveformCache&& other) noexcept {
            if (this != &other) {
                cleanup();
                rustMapping = other.rustMapping;
                valid.store(other.valid.load());
                other.rustMapping = nullptr;
                other.valid.store(false);
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

    // PDC is project/session state. It must not be shared through the legacy
    // process-wide compatibility singleton. Rust (native_pdc.rs) owns the
    // configuration, solving, thread ownership, and atomic publication; this
    // member is only the handle the control thread edits and reads from.
    void* m_pdcState = hirari_native_pdc_create();
    // Node limits checked before a value crosses into Rust. Rust keeps the
    // authoritative copies (native_pdc.rs: MAX_TRACKS, MASTER_ID); these hold
    // here so an out-of-range ID never reaches the FFI in the first place.
    static constexpr uint32_t kPdcMaxTracks = 512;
    static constexpr uint32_t kPdcMasterDestination = 0xFFFFFFFFu;
    UndoTransactionManager m_undoManager;
    TempoMap m_tempoMap;
    RoutingEngine m_routingEngine;
    SidechainManager m_sidechainManager;
    BusSystem m_busSystem;
    MacroControlManager m_macroControlManager;
    ::Hirari::Core::MidiLearnManager m_midiLearnManager;
    ::Hirari::Core::Diagnostics::EngineDiagnostics m_engineDiagnostics;
    ::Hirari::Core::Diagnostics::ForensicJournaler m_forensicJournaler;
    ::Hirari::Core::IO::AudioDecoderManager m_audioDecoderManager;
    std::atomic<bool> m_graphDirty{true};

    // Engine Infrastructure
    std::unique_ptr<ScriptManager> m_scriptManager;

    // RT-Safe Pre-allocated Kernels
    ::Hirari::Core::DSP::Effects::AtmosphericProcessorKernel m_atmosphericEngine;
    ::Hirari::Core::Mixing::MasteringKernel m_masteringKernel;
    // The Control Room model is implemented in Rust
    // (control_room_runtime.rs). This is only the handle its audio
    // callback renders with; C++ never interprets the state itself, so the
    // engine owns nothing but the lifetime.
    void* m_controlRoomState = hirari_control_room_state_create();
    ::Hirari::Core::Mixing::TrackPresetStore m_trackPresets;
    ::Hirari::Core::Plugins::PluginCompatibilityRegistry m_pluginCompatibility;
    ::Hirari::Core::Engine::AutomationLaneGroup m_automationLanes;
    ::Hirari::Core::Engine::TapTempo m_tapTempo;
    ::Hirari::Core::MIDI::ExternalSync m_externalSync;
    ::Hirari::Core::Composition::ArrangementEditModel m_arrangement;
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
    struct RegionImportJob {
        std::atomic<uint8_t> status{1}; // 1 decoding, 2 ready, 3 committed, 4 failed/stale
        std::weak_ptr<Track> target;
        uint32_t trackId = 0;
        uint64_t startSample = 0;
        std::string path;
        std::shared_ptr<::Hirari::Core::AudioBuffer> audio;
        double sourceSampleRate = 0.0;
    };
    mutable std::mutex m_regionImportJobsMutex;
    std::unordered_map<uint64_t, std::shared_ptr<RegionImportJob>> m_regionImportJobs;
    std::atomic<uint64_t> m_nextRegionImportTicket{1};
    struct AudioPitchAnalysisJob {
        std::atomic<uint8_t> status{0}; // 1 running, 2 ready to commit, 3 committed, 4 failed
        std::weak_ptr<Track> target;
        std::vector<::hirari::editing::AudioNoteSegment> before;
        std::vector<::hirari::editing::AudioNoteSegment> after;
    };
    mutable std::mutex m_audioPitchAnalysisMutex;
    std::unordered_map<uint64_t, std::shared_ptr<AudioPitchAnalysisJob>> m_audioPitchAnalysisJobs;
    struct RegionQuantizeTarget {
        std::shared_ptr<Track> track;
        const void* snapshot = nullptr;
        Region region;
        std::vector<Region::WarpMarker> before;
    };
    struct RegionQuantizeJob {
        std::atomic<uint8_t> status{0}; // 1 analyzing, 2 ready to apply, 3 applied, 4 failed/stale
        std::vector<RegionQuantizeTarget> targets;
        // Each target can have a different native source-frame count while
        // sharing one project-timeline quantization map.
        std::vector<std::vector<Region::WarpMarker>> after;
        std::vector<TempoMap::Event> tempoEvents;
        double baseTempo = 120.0;
        double sampleRate = 0.0;
    };
    mutable std::mutex m_regionQuantizeMutex;
    std::unordered_map<uint64_t, std::shared_ptr<RegionQuantizeJob>> m_regionQuantizeJobs;
    struct RegionAlignmentJob {
        std::atomic<uint8_t> status{1}; // 1 analyzing, 2 ready to apply, 3 applied, 4 failed/stale
        std::shared_ptr<Track> referenceTrack;
        uint64_t referenceRegionGeneration = 0;
        Region referenceRegion;
        struct Target {
            std::shared_ptr<Track> track;
            uint64_t regionGeneration = 0;
            Region region;
            std::vector<Region::WarpMarker> before;
            std::vector<Region::WarpMarker> after;
        };
        std::vector<Target> targets;
    };
    mutable std::mutex m_regionAlignmentMutex;
    std::unordered_map<uint64_t, std::shared_ptr<RegionAlignmentJob>> m_regionAlignmentJobs;

    // Consults the authoritative Rust scalar-edit policy. The engine layer uses
    // this only to validate input and to recover the clamped value for undo
    // snapshots, so it can never disagree with the storage layer in Track.
    HirariRegionScalarDecision decideScalarEdit(
        HirariRegionScalarField field, float valueA, float valueB = 0.0f) const {
        HirariRegionScalarEdit edit{};
        edit.field = field;
        edit.locked = false;
        edit.value_a = valueA;
        edit.value_b = valueB;
        // LoopCount/SyncGroup are range-checked without geometry; the storage
        // layer re-checks the u64 overflow guard with the real region extent.
        edit.region_len = 0;
        edit.region_start = 0;
        HirariRegionScalarDecision decision{};
        hirari_region_scalar_edit_decision(edit, &decision);
        return decision;
    }

    bool commit_region_import(uint32_t trackId, uint64_t startSample,
                              const std::string& path,
                              const std::shared_ptr<Track>& expectedTrack,
                              std::shared_ptr<::Hirari::Core::AudioBuffer> audio,
                              double sourceSampleRate);
};
