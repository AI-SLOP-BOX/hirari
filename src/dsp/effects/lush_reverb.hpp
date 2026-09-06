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
    explicit LushReverb(double sampleRate = 44100.0) : m_sampleRate(44100.0) {
        setSampleRate(sampleRate);
    }

    std::string getName() const override { return "Lush Reverb"; }

    void setupDelays() {
        // Multi-prime delay lengths at the reference rate; scale in seconds
        // so the reverb character remains consistent at 48/96/192 kHz.
        constexpr std::array<int, 8> base = {1117, 1373, 1601, 2111, 2711, 3121, 3701, 4127};
        const double scale = m_sampleRate / 44100.0;
        for (int i = 0; i < 8; ++i) {
            const size_t length = std::max<size_t>(1u, static_cast<size_t>(std::llround(base[i] * scale)));
            m_delayLines[i].assign(length, 0.0f);
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
            const float outL = inL * 0.75f + wetL * 0.03125f;
            const float outR = inR * 0.75f + wetR * 0.03125f;
            if (r == l) {
                const float mono = 0.5f * (outL + outR);
                l[s] = std::isfinite(mono) ? std::clamp(mono, -16.0f, 16.0f) : 0.0f;
            } else {
                l[s] = std::isfinite(outL) ? std::clamp(outL, -16.0f, 16.0f) : 0.0f;
                r[s] = std::isfinite(outR) ? std::clamp(outR, -16.0f, 16.0f) : 0.0f;
            }
        }
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override { (void)bs; setSampleRate(sr); reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi; (void)context;
        if (m_bypassed || buffer.getNumChannels() == 0) return;
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
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44100.0;
        setupDelays();
    }

    uint32_t getLatencySamples() const noexcept override { return 0; }
    uint32_t getTailSamples() const noexcept override {
        // At 0.85 feedback, roughly 48 longest-delay traversals reach the
        // practical -60 dB floor used by offline renderers.
        size_t longest = 0;
        for (const auto& line : m_delayLines) longest = std::max(longest, line.size());
        return static_cast<uint32_t>(std::min<size_t>(longest * 48u,
            static_cast<size_t>(m_sampleRate * 30.0)));
    }

private:
    double m_sampleRate;
    std::array<std::vector<float>, 8> m_delayLines;
    std::array<size_t, 8> m_writeIndices;
    std::array<float, 8> m_filterState = {0};
    float m_feedback = 0.85f;
    float m_damping = 0.2f;
};

} // namespace Aura::DSP::Effects
