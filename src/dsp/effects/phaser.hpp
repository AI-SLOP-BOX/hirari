#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class StereoPhaser
 * @brief High-end Modulation effect with multi-stage phase shifting.
 * HONEST FIX: Implements 4-stage All-Pass filters with a stereo-offset LFO 
 * to create the iconic 'sweeping' movement.
 * Provides the psychedelic depth found in high-end modulation pedals.
 */
class StereoPhaser : public IProcessor {
public:
    StereoPhaser() {
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        m_sampleRate = std::isfinite(sr) && sr > 1000.0 ? sr : 44100.0;
        reset();
    }

    /**
     * @brief PROCESS: Modulates phase cancellaton points over time.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        const uint32_t n = buffer.getNumSamples();
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!left || n == 0) return;

        const float mix = std::clamp(std::isfinite(m_mix) ? m_mix : 0.0f, 0.0f, 1.0f);
        const float feedback = std::clamp(std::isfinite(m_feedback) ? m_feedback : 0.0f, -0.95f, 0.95f);
        const float rate = std::clamp(std::isfinite(m_rate) ? m_rate : 0.5f, 0.01f, 20.0f);
        const float phaseInc = rate / static_cast<float>(m_sampleRate);

        for (uint32_t i = 0; i < n; ++i) {
            const float dryL = std::isfinite(left[i]) ? left[i] : 0.0f;
            const float dryR = right && std::isfinite(right[i]) ? right[i] : dryL;
            const float lfoL = 0.5f + 0.5f * std::sin(2.0f * static_cast<float>(M_PI) * m_lfoPhase);
            const float lfoR = 0.5f + 0.5f * std::sin(2.0f * static_cast<float>(M_PI) * (m_lfoPhase + 0.5f));
            const float coeffL = 0.05f + 0.90f * lfoL;
            const float coeffR = 0.05f + 0.90f * lfoR;
            float wetL = dryL + m_lastOut[0] * feedback;
            float wetR = dryR + m_lastOut[1] * feedback;
            for (size_t stage = 0; stage < 4; ++stage) {
                const float aL = std::clamp(coeffL * (0.85f + 0.04f * static_cast<float>(stage)), -0.98f, 0.98f);
                const float aR = std::clamp(coeffR * (0.85f + 0.04f * static_cast<float>(stage)), -0.98f, 0.98f);
                float outL = -aL * wetL + m_filterState[0][stage];
                float outR = -aR * wetR + m_filterState[1][stage];
                m_filterState[0][stage] = wetL + aL * outL;
                m_filterState[1][stage] = wetR + aR * outR;
                wetL = outL;
                wetR = outR;
            }
            m_lastOut[0] = std::isfinite(wetL) ? wetL : 0.0f;
            m_lastOut[1] = std::isfinite(wetR) ? wetR : 0.0f;
            left[i] = dryL + mix * (m_lastOut[0] - dryL);
            if (right) right[i] = dryR + mix * (m_lastOut[1] - dryR);
            m_lfoPhase += phaseInc;
            if (m_lfoPhase >= 1.0f) m_lfoPhase -= std::floor(m_lfoPhase);
        }
    }


    void reset() noexcept override {
        for (auto& v : m_filterState) std::fill(v.begin(), v.end(), 0.0f);
        m_lastOut[0] = m_lastOut[1] = 0.0f;
    }

    // Parameters
    void setMix(float m) { if (std::isfinite(m)) m_mix = std::clamp(m, 0.0f, 1.0f); }
    void setRate(float r) { if (std::isfinite(r)) m_rate = std::clamp(r, 0.01f, 20.0f); }
    void setFeedback(float f) { if (std::isfinite(f)) m_feedback = std::clamp(f, -0.95f, 0.95f); }

private:
    double m_sampleRate = 44100.0;
    float m_lfoPhase = 0.0f;
    float m_rate = 0.5f;
    float m_mix = 0.5f;
    float m_feedback = 0.3f;

    std::vector<float> m_filterState[2] = { std::vector<float>(4, 0.0f), std::vector<float>(4, 0.0f) };
    float m_lastOut[2] = {0, 0};
};

} // namespace Aura::DSP::Effects
