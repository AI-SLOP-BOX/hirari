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

    struct AutomationPoint {
        double time = 0.0;
        float value = 0.0f;
        float curve = 0.0f;
    };

    static AuraUnifiedEngine& getInstance();
    AuraUnifiedEngine();
    ~AuraUnifiedEngine();

    void apply_config(const EngineConfig& cfg);
    void processBlock(::Aura::Core::AudioBuffer& output, uint32_t offset, uint32_t size);
    void processBlockDirect(float** buffers, uint32_t numChannels, uint32_t numSamples);
    // Publishes the hardware input block for the realtime input-monitor path.
    // The caller owns the input buffers; this method copies into fixed storage
    // so the graph never retains driver pointers beyond the callback.
    void publish_input_monitor_block(const float* const* inputs,
                                     uint32_t numChannels,
                                     uint32_t numSamples) noexcept;
    // Shared-map transient quantization for phase-coherent multi-mic groups.
    // This is deliberately exposed at the engine boundary so offline editors
    // and realtime clients use the identical warp implementation.
    bool quantizeAudioGroup(float* const* buffers, uint32_t channels,
                            uint64_t length, float bpm, double sampleRate,
                            float strength = 1.0f, float swing = 0.0f) {
        if (!buffers || channels == 0 || channels > 32) return false;
        std::array<const float*, 32> inputs{};
        std::array<float*, 32> outputs{};
        for (uint32_t i = 0; i < channels; ++i) {
            inputs[i] = buffers[i];
            outputs[i] = buffers[i];
        }
        return ::Aura::Core::Engine::AudioQuantizer::quantizeGroup(
            inputs.data(), outputs.data(), channels, length, bpm, sampleRate,
            {strength, swing});
    }
    // Compatibility entry point for older native integrations.  Keep the
    // raw-buffer API routed through the same validated processing path as the
    // packaged application instead of maintaining a second DSP implementation.
    void renderBlock(float* outL, float* outR, uint32_t numSamples,
                     const EngineContext& context) {
        (void)context;
        if (outL == nullptr || outR == nullptr || numSamples == 0 ||
            numSamples > kMaxAudioBlockSize) {
            if (outL != nullptr && outR != nullptr && numSamples > 0 &&
                numSamples <= kMaxAudioBlockSize) {
                std::fill_n(outL, numSamples, 0.0f);
                std::fill_n(outR, numSamples, 0.0f);
            }
            return;
        }
        float* channels[2] = {outL, outR};
        processBlockDirect(channels, 2, numSamples);
    }
    void waitForAudioCallbacks() const noexcept;
    void prepareToPlay(double sr, uint32_t sz);
    void syncStructuralChanges();

