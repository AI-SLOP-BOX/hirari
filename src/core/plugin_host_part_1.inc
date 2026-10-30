
class NativeProcessor final : public DSP::IProcessor {
public:
    explicit NativeProcessor(int32_t kind)
        : m_kind(kind), m_state(hirari_native_processor_create(kind)) {}
    ~NativeProcessor() override { hirari_native_processor_destroy(m_kind, m_state); }
    NativeProcessor(const NativeProcessor&) = delete;
    NativeProcessor& operator=(const NativeProcessor&) = delete;
    void prepareToPlay(double sampleRate, uint32_t) noexcept override {
        hirari_native_processor_prepare(m_kind, m_state, sampleRate);
    }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext& context) noexcept override {
        hirari_native_processor_process(m_kind, m_state, buffer.getArrayOfWritePointers(),
            buffer.getNumChannels(), buffer.getNumSamples(), context.bpm);
    }
    void reset() noexcept override { hirari_native_processor_reset(m_kind, m_state); }
    std::string getName() const override {
        const char* name = hirari_native_processor_name(m_kind);
        return name ? std::string(name) : std::string("Hirari Native");
    }
    uint32_t getTailSamples() const noexcept override {
        return hirari_native_processor_tail_samples(m_kind, m_state);
    }
    uint32_t getLatencySamples() const noexcept override {
        return hirari_native_processor_latency_samples(m_kind, m_state);
    }
    uint32_t getNumParameters() const noexcept override {
        return hirari_native_processor_parameter_count(m_kind);
    }
    void setParameter(uint32_t id, float value) noexcept override {
        hirari_native_processor_set_parameter(m_kind, m_state, id, value);
    }
    float getParameter(uint32_t id) const noexcept override {
        return hirari_native_processor_get_parameter(m_kind, m_state, id);
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        std::snprintf(outName, maxSize, "%s",
                      hirari_native_processor_parameter_name(m_kind, id));
    }
    // Kinds with their own persisted format serialize it in Rust; the shared
    // 16-byte host envelope remains the fallback for everything else.
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(32, 0);
        const size_t written = hirari_native_processor_state_save(
            m_kind, m_state, m_bypassed ? 1u : 0u, m_mix, m_sidechainBusId,
            state.data(), state.size());
        if (written == 0) return IProcessor::getState();
        state.resize(written);
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        uint8_t bypassed = 0;
        float mix = 1.0f;
        uint32_t sidechain = 0;
        if (!hirari_native_processor_state_restore(m_kind, m_state, state.data(),
                                                   state.size(), &bypassed, &mix,
                                                   &sidechain)) {
            return IProcessor::setState(state);
        }
        m_bypassed = bypassed != 0;
        m_mix = mix;
        m_sidechainBusId = sidechain;
        return true;
    }
private:
    int32_t m_kind;
    void* m_state = nullptr;
};
