
class NativeNoiseGate final : public DSP::IProcessor {
public:
    void prepareToPlay(double sr, uint32_t) noexcept override { m_sampleRate = sr > 0.0 ? sr : 44100.0; }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        if (channels == 0) return;
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            float level = 0.0f;
            for (uint32_t c = 0; c < channels; ++c) level = std::max(level, std::abs(buffer.getReadPointer(c)[i]));
            const float target = level >= m_threshold ? 1.0f : 0.0f;
            m_gain += (target - m_gain) * (target > m_gain ? m_attack : m_release);
            for (uint32_t c = 0; c < channels; ++c) buffer.getWritePointer(c)[i] *= m_gain;
        }
    }
    void reset() noexcept override { m_gain = 0.0f; }
    std::string getName() const override { return "Aura Gate"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) m_threshold = std::clamp(value, 0.00001f, 1.0f); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_threshold : 0.0f; }
private:
    double m_sampleRate = 44100.0;
    float m_threshold = 0.01f, m_gain = 0.0f;
    float m_attack = 0.05f, m_release = 0.005f;
};

class NativeTransientShaper final : public DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override { reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            float level = 0.0f;
            for (uint32_t c = 0; c < channels; ++c) level = std::max(level, std::abs(buffer.getReadPointer(c)[i]));
            const float transient = std::max(0.0f, level - m_envelope);
            m_envelope += (level - m_envelope) * 0.08f;
            const float boost = 1.0f + std::min(2.0f, transient * m_amount * 8.0f);
            for (uint32_t c = 0; c < channels; ++c) buffer.getWritePointer(c)[i] = std::clamp(buffer.getReadPointer(c)[i] * boost, -1.0f, 1.0f);
        }
    }
    void reset() noexcept override { m_envelope = 0.0f; }
    std::string getName() const override { return "Aura Transient Shaper"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) m_amount = std::clamp(value, 0.0f, 1.0f); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_amount : 0.0f; }
private:
    float m_envelope = 0.0f, m_amount = 0.5f;
};

class NativeDeEsser final : public DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override { reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            float level = 0.0f;
            for (uint32_t c = 0; c < channels; ++c) level = std::max(level, std::abs(buffer.getReadPointer(c)[i]));
            m_detector += (level - m_detector) * 0.12f;
            const float excess = std::max(0.0f, m_detector - m_threshold);
            const float gain = std::clamp(1.0f - excess * m_amount * 2.5f, 0.15f, 1.0f);
            for (uint32_t c = 0; c < channels; ++c) buffer.getWritePointer(c)[i] *= gain;
        }
    }
    void reset() noexcept override { m_detector = 0.0f; }
    std::string getName() const override { return "Aura De-Esser"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) m_amount = std::clamp(value, 0.0f, 1.0f); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_amount : 0.0f; }
private:
    float m_detector = 0.0f, m_amount = 0.5f, m_threshold = 0.45f;
};

class NativeDelay final : public DSP::IProcessor {
public:
    void prepareToPlay(double sr, uint32_t) noexcept override { m_sampleRate = sr > 0.0 ? sr : 44100.0; reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext& context) noexcept override {
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        const uint32_t delay = std::clamp<uint32_t>(static_cast<uint32_t>(m_sampleRate * 60.0 / (std::max(1.0, context.bpm) * 2.0)), 1, kSize - 1);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const uint32_t read = (m_write + kSize - delay) % kSize;
            for (uint32_t c = 0; c < channels; ++c) {
                const float input = buffer.getReadPointer(c)[i];
                const float echo = m_buffer[c][read];
                m_buffer[c][m_write] = input + echo * m_feedback;
                buffer.getWritePointer(c)[i] = input * (1.0f - m_mix) + echo * m_mix;
            }
            ++m_write;
            if (m_write == kSize) m_write = 0;
        }
    }
    void reset() noexcept override { m_write = 0; for (auto& channel : m_buffer) channel.fill(0.0f); }
    std::string getName() const override { return "Aura Delay"; }
    // Eight delay cycles leave the 0.35 feedback path below -60 dB even at
    // the longest supported tempo-derived delay.
    uint32_t getTailSamples() const noexcept override { return kSize * 8u; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) m_mix = std::clamp(value, 0.0f, 1.0f); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_mix : 0.0f; }
private:
    static constexpr uint32_t kSize = 65536;
    double m_sampleRate = 44100.0;
    std::array<std::array<float, kSize>, 2> m_buffer{};
    uint32_t m_write = 0;
    float m_feedback = 0.35f, m_mix = 0.35f;
};

class NativeReverb final : public DSP::IProcessor {
public:
    void prepareToPlay(double sr, uint32_t) noexcept override {
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0
            ? sr : 44'100.0;
        reset();
    }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const float inL = buffer.getReadPointer(0)[i];
            const float inR = channels > 1 ? buffer.getReadPointer(1)[i] : inL;
            const float wetL = m_lines[0][m_index] + m_lines[2][m_index];
            const float wetR = m_lines[1][m_index] + m_lines[3][m_index];
            m_lines[0][m_index] = inL + wetR * m_feedback;
            m_lines[1][m_index] = inR + wetL * m_feedback;
            m_lines[2][m_index] = inL * 0.7f + wetL * m_feedback * 0.6f;
            m_lines[3][m_index] = inR * 0.7f + wetR * m_feedback * 0.6f;
            buffer.getWritePointer(0)[i] = inL * (1.0f - m_mix) + wetL * m_mix * 0.5f;
            if (channels > 1) buffer.getWritePointer(1)[i] = inR * (1.0f - m_mix) + wetR * m_mix * 0.5f;
            m_index = (m_index + 1) % kSize;
        }
    }
    void reset() noexcept override { m_index = 0; for (auto& line : m_lines) line.fill(0.0f); }
    std::string getName() const override { return "Aura Reverb"; }
    // The four-line tank feeds back at 0.72; reserving 24 maximum-delay
    // traversals reaches the practical -60 dB decay point with margin.
    uint32_t getTailSamples() const noexcept override { return kSize * 24u; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) m_mix = std::clamp(value, 0.0f, 1.0f); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_mix : 0.0f; }
private:
    static constexpr uint32_t kSize = 8192;
    std::array<std::array<float, kSize>, 4> m_lines{};
    uint32_t m_index = 0;
    double m_sampleRate = 44'100.0;
    float m_feedback = 0.72f, m_mix = 0.3f;
};

class NativeDynamicEq final : public DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override { reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            float level = 0.0f;
            for (uint32_t c = 0; c < channels; ++c) level = std::max(level, std::abs(buffer.getReadPointer(c)[i]));
            m_env += (level - m_env) * 0.1f;
            const float reduction = m_env > m_threshold ? std::clamp((m_env - m_threshold) * m_amount, 0.0f, 0.75f) : 0.0f;
            for (uint32_t c = 0; c < channels; ++c) buffer.getWritePointer(c)[i] *= 1.0f - reduction;
        }
    }
    void reset() noexcept override { m_env = 0.0f; }
    std::string getName() const override { return "Aura Dynamic EQ"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) m_amount = std::clamp(value, 0.0f, 1.0f); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_amount : 0.0f; }
private:
    float m_env = 0.0f, m_amount = 0.5f, m_threshold = 0.35f;
};

class NativeMidSide final : public DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        if (buffer.getNumChannels() < 2) return;
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const float l = buffer.getReadPointer(0)[i], r = buffer.getReadPointer(1)[i];
            const float mid = (l + r) * 0.70710678f * m_mid;
            const float side = (l - r) * 0.70710678f * m_side;
            buffer.getWritePointer(0)[i] = (mid + side) * 0.70710678f;
            buffer.getWritePointer(1)[i] = (mid - side) * 0.70710678f;
        }
    }
    void reset() noexcept override {}
    std::string getName() const override { return "Aura Mid/Side"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) m_side = std::clamp(value, 0.0f, 1.0f) * 2.0f; }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_side * 0.5f : 0.0f; }
private:
    float m_mid = 1.0f, m_side = 1.0f;
};

class NativeStereoWidth final : public DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        if (buffer.getNumChannels() < 2) return;
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const float l = buffer.getReadPointer(0)[i], r = buffer.getReadPointer(1)[i];
            const float mid = (l + r) * 0.5f, side = (l - r) * 0.5f * m_width;
            buffer.getWritePointer(0)[i] = mid + side;
            buffer.getWritePointer(1)[i] = mid - side;
        }
    }
    void reset() noexcept override {}
    std::string getName() const override { return "Aura Stereo Width"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) m_width = std::clamp(value, 0.0f, 1.0f) * 2.0f; }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_width * 0.5f : 0.0f; }
private:
    float m_width = 1.0f;
};

