
class NativeNoiseGate final : public DSP::IProcessor {
public:
    NativeNoiseGate() : m_state(hirari_native_noise_gate_create()) {}
    ~NativeNoiseGate() override { hirari_native_noise_gate_destroy(m_state); }
    NativeNoiseGate(const NativeNoiseGate&) = delete;
    NativeNoiseGate& operator=(const NativeNoiseGate&) = delete;
    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        hirari_native_noise_gate_process(m_state, buffer.getArrayOfWritePointers(),
            buffer.getNumChannels(), buffer.getNumSamples());
    }
    void reset() noexcept override { hirari_native_noise_gate_reset(m_state); }
    std::string getName() const override { return "Hirari Gate"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) hirari_native_noise_gate_set_threshold(m_state, value); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? hirari_native_noise_gate_get_threshold(m_state) : 0.0f; }
private:
    void* m_state = nullptr;
};

class NativeTransientShaper final : public DSP::IProcessor {
public:
    NativeTransientShaper() : m_state(hirari_native_transient_shaper_create()) {}
    ~NativeTransientShaper() override { hirari_native_transient_shaper_destroy(m_state); }
    NativeTransientShaper(const NativeTransientShaper&) = delete;
    NativeTransientShaper& operator=(const NativeTransientShaper&) = delete;
    void prepareToPlay(double, uint32_t) noexcept override { reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        hirari_native_transient_shaper_process(m_state, buffer.getArrayOfWritePointers(),
            buffer.getNumChannels(), buffer.getNumSamples());
    }
    void reset() noexcept override { hirari_native_transient_shaper_reset(m_state); }
    std::string getName() const override { return "Hirari Transient Shaper"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) hirari_native_transient_shaper_set_amount(m_state, value); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? hirari_native_transient_shaper_get_amount(m_state) : 0.0f; }
private:
    void* m_state = nullptr;
};

class NativeDeEsser final : public DSP::IProcessor {
public:
    NativeDeEsser() : m_state(hirari_native_deesser_create()) {}
    ~NativeDeEsser() override { hirari_native_deesser_destroy(m_state); }
    NativeDeEsser(const NativeDeEsser&) = delete;
    NativeDeEsser& operator=(const NativeDeEsser&) = delete;
    void prepareToPlay(double, uint32_t) noexcept override { reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        hirari_native_deesser_process(m_state, buffer.getArrayOfWritePointers(),
            buffer.getNumChannels(), buffer.getNumSamples());
    }
    void reset() noexcept override { hirari_native_deesser_reset(m_state); }
    std::string getName() const override { return "Hirari De-Esser"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) hirari_native_deesser_set_amount(m_state, value); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? hirari_native_deesser_get_amount(m_state) : 0.0f; }
private:
    void* m_state = nullptr;
};

class NativeDelay final : public DSP::IProcessor {
public:
    NativeDelay() : m_state(hirari_native_delay_create()) {}
    ~NativeDelay() override { hirari_native_delay_destroy(m_state); }
    NativeDelay(const NativeDelay&) = delete;
    NativeDelay& operator=(const NativeDelay&) = delete;
    void prepareToPlay(double sr, uint32_t) noexcept override { hirari_native_delay_prepare(m_state, sr); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext& context) noexcept override {
        hirari_native_delay_process(m_state, buffer.getArrayOfWritePointers(),
            buffer.getNumChannels(), buffer.getNumSamples(), context.bpm);
    }
    void reset() noexcept override { hirari_native_delay_reset(m_state); }
    std::string getName() const override { return "Hirari Delay"; }
    // Eight delay cycles leave the 0.35 feedback path below -60 dB even at
    // the longest supported tempo-derived delay.
    uint32_t getTailSamples() const noexcept override { return 65536u * 8u; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) hirari_native_delay_set_mix(m_state, value); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? hirari_native_delay_get_mix(m_state) : 0.0f; }
private:
    void* m_state = nullptr;
};

class NativeReverb final : public DSP::IProcessor {
public:
    NativeReverb() : m_state(hirari_native_reverb_create()) {}
    ~NativeReverb() override { hirari_native_reverb_destroy(m_state); }
    NativeReverb(const NativeReverb&) = delete;
    NativeReverb& operator=(const NativeReverb&) = delete;
    void prepareToPlay(double, uint32_t) noexcept override { reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        hirari_native_reverb_process(m_state, buffer.getArrayOfWritePointers(),
            buffer.getNumChannels(), buffer.getNumSamples());
    }
    void reset() noexcept override { hirari_native_reverb_reset(m_state); }
    std::string getName() const override { return "Hirari Reverb"; }
    // The four-line tank feeds back at 0.72; reserving 24 maximum-delay
    // traversals reaches the practical -60 dB decay point with margin.
    uint32_t getTailSamples() const noexcept override { return 8192u * 24u; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) hirari_native_reverb_set_mix(m_state, value); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? hirari_native_reverb_get_mix(m_state) : 0.0f; }
private:
    void* m_state = nullptr;
};

class NativeDynamicEq final : public DSP::IProcessor {
public:
    NativeDynamicEq() : m_state(hirari_native_dynamic_eq_create()) {}
    ~NativeDynamicEq() override { hirari_native_dynamic_eq_destroy(m_state); }
    NativeDynamicEq(const NativeDynamicEq&) = delete;
    NativeDynamicEq& operator=(const NativeDynamicEq&) = delete;
    void prepareToPlay(double, uint32_t) noexcept override { reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        hirari_native_dynamic_eq_process(m_state, buffer.getArrayOfWritePointers(),
            buffer.getNumChannels(), buffer.getNumSamples());
    }
    void reset() noexcept override { hirari_native_dynamic_eq_reset(m_state); }
    std::string getName() const override { return "Hirari Dynamic EQ"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) hirari_native_dynamic_eq_set_amount(m_state, value); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? hirari_native_dynamic_eq_get_amount(m_state) : 0.0f; }
private:
    void* m_state = nullptr;
};

class NativeMidSide final : public DSP::IProcessor {
public:
    NativeMidSide() : m_state(hirari_native_mid_side_create()) {}
    ~NativeMidSide() override { hirari_native_mid_side_destroy(m_state); }
    NativeMidSide(const NativeMidSide&) = delete;
    NativeMidSide& operator=(const NativeMidSide&) = delete;
    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        hirari_native_mid_side_process(m_state, buffer.getArrayOfWritePointers(),
            buffer.getNumChannels(), buffer.getNumSamples());
    }
    void reset() noexcept override {}
    std::string getName() const override { return "Hirari Mid/Side"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) hirari_native_mid_side_set(m_state, value); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? hirari_native_mid_side_get(m_state) : 0.0f; }
private:
    void* m_state = nullptr;
};

class NativeStereoWidth final : public DSP::IProcessor {
public:
    NativeStereoWidth() : m_state(hirari_native_stereo_width_create()) {}
    ~NativeStereoWidth() override { hirari_native_stereo_width_destroy(m_state); }
    NativeStereoWidth(const NativeStereoWidth&) = delete;
    NativeStereoWidth& operator=(const NativeStereoWidth&) = delete;
    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        hirari_native_stereo_width_process(m_state, buffer.getArrayOfWritePointers(),
            buffer.getNumChannels(), buffer.getNumSamples());
    }
    void reset() noexcept override {}
    std::string getName() const override { return "Hirari Stereo Width"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) hirari_native_stereo_width_set(m_state, value); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? hirari_native_stereo_width_get(m_state) : 0.0f; }
private:
    void* m_state = nullptr;
};
