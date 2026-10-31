public:
    // One block-size contract shared by the native graph and every bridge
    // entry point. Keeping this in the public engine boundary prevents the
    // FFI from silently rejecting a block size that the configured engine can
    // process, or accepting a size the graph does not bound.
    static constexpr uint32_t kMaxAudioBlockSize = 16'384u;

    enum class BounceState : uint32_t {
        Idle = 0,
        Queued = 1,
        Rendering = 2,
        Completed = 3,
        Failed = 4,
        Cancelled = 5,
        Paused = 6,
    };

    static HirariUnifiedEngine& getInstance();
    HirariUnifiedEngine();
    ~HirariUnifiedEngine();

    void apply_config(const EngineConfig& cfg);
    void processBlock(::Hirari::Core::AudioBuffer& output, uint32_t offset, uint32_t size);
    void processBlockDirect(float** buffers, uint32_t numChannels, uint32_t numSamples);
    // Publishes the hardware input block for the realtime input-monitor path.
    // The caller owns the input buffers; this method copies into fixed storage
    // so the graph never retains driver pointers beyond the callback.
    void publish_input_monitor_block(const float* const* inputs,
                                     uint32_t numChannels,
                                     uint32_t numSamples) noexcept;
    // Publishes a mono hardware input snapshot for the dedicated Control
    // Room talkback path. The audio graph never retains driver-owned buffers.
    void publish_talkback_input_block(const float* const* inputs,
                                      uint32_t numChannels,
                                      uint32_t numSamples) noexcept;
    void waitForAudioCallbacks() const noexcept;
    void prepareToPlay(double sr, uint32_t sz);
    void syncStructuralChanges();
