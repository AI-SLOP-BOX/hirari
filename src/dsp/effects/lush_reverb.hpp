#pragma once

#include <vector>
#include <cmath>
#include <array>
#include <algorithm>
#include "../iprocessor.hpp"
#include "../math/denormal_killer.hpp"
#include "../../core/audio_buffer.hpp"

namespace Aura::DSP::Effects {

/**
 * @brief LushReverb: High-end Algorithmic Feedback Delay Network (FDN).
 */
class LushReverb : public IProcessor {
public:
    explicit LushReverb(double sampleRate = 44100.0) : m_sampleRate(sampleRate > 1000.0 ? sampleRate : 44100.0) {
        setupDelays();
    }

    void setupDelays() {
        // Multi-prime delay lengths for high modal density (8x8 FDN)
        std::array<int, 8> len = {1117, 1373, 1601, 2111, 2711, 3121, 3701, 4127};
        for (int i = 0; i < 8; ++i) {
            m_delayLines[i].assign(len[i], 0.0f);
            m_writeIndices[i] = 0;
        }
    }

    void process(float* l, float* r, uint32_t numSamples) {
        if (!l || !r) return;
        const float feedback = std::clamp(m_feedback, 0.0f, 0.995f);
        const float damping = std::clamp(m_damping, 0.0f, 0.99f);
        for (uint32_t s = 0; s < numSamples; ++s) {
            const float inL = std::isfinite(l[s]) ? l[s] : 0.0f;
            const float inR = std::isfinite(r[s]) ? r[s] : 0.0f;
            const float input = 0.5f * (inL + inR);
            float sum = 0.0f;
            for (size_t i = 0; i < m_delayLines.size(); ++i) {
                auto& line = m_delayLines[i];
                const size_t read = (m_writeIndices[i] + 1) % line.size();
                const float delayed = std::isfinite(line[read]) ? line[read] : 0.0f;
                m_filterState[i] += (delayed - m_filterState[i]) * (1.0f - damping);
                sum += m_filterState[i];
            }
            const float mean = sum / 8.0f;
            float wetL = 0.0f, wetR = 0.0f;
            for (size_t i = 0; i < m_delayLines.size(); ++i) {
                auto& line = m_delayLines[i];
                const float diffuse = m_filterState[i] - 2.0f * mean;
                line[m_writeIndices[i]] = std::isfinite(input + feedback * diffuse) ? input + feedback * diffuse : 0.0f;
                m_writeIndices[i] = (m_writeIndices[i] + 1) % line.size();
                if ((i & 1u) == 0) wetL += m_filterState[i]; else wetR += m_filterState[i];
            }
            l[s] = inL * 0.75f + wetL * 0.03125f;
            r[s] = inR * 0.75f + wetR * 0.03125f;
        }
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override { (void)bs; setSampleRate(sr); reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi; (void)context;
        if (buffer.getNumChannels() == 0) return;
        float* l = buffer.getWritePointer(0);
        float* r = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : l;
        process(l, r, buffer.getNumSamples());
    }
    void reset() noexcept override {
        for (auto& line : m_delayLines) std::fill(line.begin(), line.end(), 0.0f);
        m_filterState.fill(0.0f);
        m_writeIndices.fill(0);
    }


    void setSampleRate(double sr) {
        m_sampleRate = std::isfinite(sr) && sr > 1000.0 ? sr : 44100.0;
        setupDelays();
    }

    uint32_t getLatency() const { return 0; }

private:
    double m_sampleRate;
    std::array<std::vector<float>, 8> m_delayLines;
    std::array<size_t, 8> m_writeIndices;
    std::array<float, 8> m_filterState = {0};
    float m_feedback = 0.85f;
    float m_damping = 0.2f;
};

} // namespace Aura::DSP::Effects
