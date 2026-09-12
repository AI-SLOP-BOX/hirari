public:
    struct GenerationContext {
        uint64_t project = 0;
        uint64_t plugin = 0;
        uint64_t audio = 0;
        uint64_t state = 0;
        bool valid() const noexcept {
            return project != 0 && plugin != 0 && audio != 0 && state != 0;
        }
    };
    enum class Failure : uint8_t {
        None,
        MissingHelper,
        InvalidPluginPath,
        SpawnFailed,
        HandshakeTimeout,
        PluginLoadFailed,
        PluginAbiInvalid,
        PluginInstanceFailed,
        SecuritySetupFailed,
        PluginFormatUnsupported,
        PluginQuarantined,
        ProcessExited,
        ProcessHung,
        ProcessFailed,
        UnsupportedPlatform,
    };

    PluginSandboxHost(const std::string& pluginPath, double sampleRate = 44100.0,
                      uint32_t maxBlockSize = 512, uint32_t channels = SandboxProtocol::kMaxChannels,
                      std::string requestedFormat = "auto")
        : m_pluginPath(pluginPath),
          m_requestedFormat(std::move(requestedFormat)),
          m_sampleRate(std::isfinite(sampleRate) && sampleRate > 0.0 ? sampleRate : 44100.0),
          m_maxBlockSize(std::clamp(maxBlockSize, 1u, SandboxProtocol::kMaxFrames)),
          m_channels(std::clamp(channels, 1u, SandboxProtocol::kMaxChannels)),
          m_processAlive(false) {}

    ~PluginSandboxHost() { stop(); }

    PluginSandboxHost(const PluginSandboxHost&) = delete;
    PluginSandboxHost& operator=(const PluginSandboxHost&) = delete;

    // Control-thread only. The helper is explicitly supplied so a missing
    // helper cannot be mistaken for a live sandbox.
