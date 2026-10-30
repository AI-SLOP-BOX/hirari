
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
        return hirari_native_processor_tail_samples(m_kind);
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
private:
    int32_t m_kind;
    void* m_state = nullptr;
};
