#include <vector>
#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"
#include "../utils/dsp_utils.hpp"

namespace Aura::DSP::Effects {

/**
 * @class AutoFilter
 * @brief Professional Dynamic Resonant Filter (Auto-Wah).
 */
class AutoFilter : public IProcessor {
public:
    AutoFilter() : m_cutoffBase(0.2f), m_res(0.3f), m_sens(0.8f), m_env(0.0f) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = sr;
        updateTimeConstants();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            float input[2] = {buffer.getReadPointer(0)[i], channels > 1 ? buffer.getReadPointer(1)[i] : buffer.getReadPointer(0)[i]};
            const float detector = 0.5f * (std::abs(input[0]) + std::abs(input[1]));
            const float coeff = detector > m_env ? m_attack : m_release;
            m_env += (detector - m_env) * coeff;
            const float cutoffNorm = std::clamp(m_cutoffBase + m_env * m_sens * 0.7f, 0.01f, 0.49f);
            const float f = 2.0f * std::sin(3.14159265f * cutoffNorm);
            const float damp = std::clamp(2.0f * (1.0f - std::pow(m_res, 0.25f)), 0.05f, 2.0f);
            for (uint32_t c = 0; c < channels; ++c) {
                const float low = m_s1[c] + f * m_s2[c];
                const float high = input[c] - low - damp * m_s2[c];
                const float band = f * high + m_s2[c];
                m_s1[c] = low;
                m_s2[c] = band;
                const float wet = low + band * 0.35f;
                buffer.getWritePointer(c)[i] = input[c] * (1.0f - m_mix) + wet * m_mix;
            }
        }
    }


    void reset() noexcept override {
        m_env = 0.0f;
        m_s1[0] = m_s1[1] = 0.0f;
        m_s2[0] = m_s2[1] = 0.0f;
    }

    void setParameter(uint32_t id, float value) noexcept override {
        if (id == 0) m_cutoffBase = std::clamp(value, 0.0f, 1.0f);
        else if (id == 1) m_res = std::clamp(value, 0.0f, 1.0f);
        else if (id == 2) m_sens = std::clamp(value, 0.0f, 1.0f);
    }

private:
    void updateTimeConstants() {
        m_attack = 1.0f - std::exp(-1.0f / (0.005f * m_sampleRate)); // 5ms
        m_release = 1.0f - std::exp(-1.0f / (0.100f * m_sampleRate)); // 100ms
    }

    double m_sampleRate = 44100.0;
    float m_cutoffBase, m_res, m_sens;
    float m_env, m_attack, m_release;
    float m_g = 0, m_k = 0;
    float m_s1[2] = {0,0}, m_s2[2] = {0,0}; // Filter states
};

} // namespace Aura::DSP::Effects
