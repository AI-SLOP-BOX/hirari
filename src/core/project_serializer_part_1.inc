public:
    // Cumulative load budgets prevent many individually-valid blobs from
    // forcing unbounded allocations in one project.
    static constexpr size_t kMaxProjectBytes = 256u * 1024u * 1024u;
    static constexpr size_t kMaxStringBytes = 64u * 1024u * 1024u;
    static constexpr size_t kMaxPluginDataBytes = 128u * 1024u * 1024u;
    static constexpr size_t kMaxPluginStateBytes = 128u * 1024u * 1024u;
    static constexpr uint64_t kMaxAutomationPoints = 4'000'000u;
    static constexpr uint64_t kMaxPluginStateEntries = 1'000'000u;

    struct AutomationPoint {
        double time = 0.0;
        float value = 0.0f;
        float curve = 0.0f;
    };
    struct PluginAutomationLane {
        uint32_t pluginIndex = 0;
        uint32_t parameterId = 0;
        std::vector<AutomationPoint> points;
    };
    struct TrackState { 
        uint32_t id; 
        uint32_t type = 0;
        float volume; 
        float pan; 
        float pan3d_x, pan3d_y, pan3d_z; 
        bool muted = false;
        bool solo = false;
        bool phaseInvert = false;
        bool recordArmed = false;
        uint32_t trackDelaySamples = 0;
        std::string pluginName; 
        std::vector<uint8_t> pluginData;
        std::vector<std::vector<uint8_t>> pluginStates;
        std::vector<std::vector<uint8_t>> pluginGuiStates;
        std::vector<uint8_t> pluginBypass;
        std::vector<std::string> sandboxedPluginPaths;
        std::vector<std::vector<uint8_t>> sandboxedPluginStates;
        std::vector<AutomationPoint> volumeAutomation;
        std::vector<AutomationPoint> panAutomation;
        std::vector<AutomationPoint> trackDelayAutomation;
        std::vector<PluginAutomationLane> pluginAutomation;
    };
    
    struct RegionState {
        struct RangeEdit { uint64_t start = 0; uint64_t end = 0; float gain = 1.0f; uint64_t fadeIn = 0; uint64_t fadeOut = 0; };
        struct WarpMarker { uint64_t sourceSample = 0; uint64_t timelineSample = 0; bool transient = false; };
        uint32_t id = 0;
        uint32_t trackId;
        uint64_t samplePosition;
        uint64_t sampleLength;
        uint64_t sourceLength = 0;
        uint32_t sourceSampleRate = 0;
        uint64_t sourceOffset = 0;
        uint64_t baseStart = 0;
        uint64_t baseSourceOffset = 0;
        uint64_t baseLength = 0;
        bool isMuted;
        std::string filePath;
        std::string name;
        float clipGain = 1.0f;
        uint64_t fadeInSamples = 64;
        uint64_t fadeOutSamples = 64;
        bool reverse = false;
        double warpRatio = 1.0;
        bool pitchPreserveWarp = false;
        float pitchSemitones = 0.0f;
        uint32_t loopCount = 1;
        std::vector<RangeEdit> rangeEdits;
        bool locked = false;
        uint32_t syncGroup = 0;
        std::vector<::hirari::editing::EventProcessingStep> processingHistory;
        std::vector<::hirari::editing::AudioNoteSegment> audioNoteSegments;
        std::vector<WarpMarker> warpMarkers;
    };

    struct SidechainState {
        uint32_t sourceId = 0;
        uint32_t destinationId = 0;
        uint32_t pluginIndex = 0;
        uint32_t tapPoint = 2;
    };

    struct RouteState {
        uint32_t sourceId = 0;
        uint32_t destinationId = 0;
        float gain = 1.0f;
        bool send = false;
        bool preFader = false;
    };
    struct MarkerState { uint64_t sample = 0; std::string name; uint32_t color = 0xff808080; };
    struct ArrangerPartState { uint64_t start = 0, length = 0; std::string name; uint32_t repeats = 1; };
    struct TempoEventState { uint64_t sample = 0; double bpm = 120.0; uint8_t ramp = 0; };

    struct ProjectState { 
        double bpm; 
        int32_t rootNote, scaleType; 
        std::vector<TrackState> tracks; 
        std::vector<RegionState> regions;
        uint32_t sampleRate;
        std::vector<RouteState> routes;
        std::vector<SidechainState> sidechains;
        std::vector<MarkerState> markers;
        std::vector<ArrangerPartState> arrangerParts;
        std::vector<TempoEventState> tempoEvents;
        uint32_t version = 42;
        bool valid = false;
    };
