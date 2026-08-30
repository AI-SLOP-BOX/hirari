#pragma once

#include <vector>
#include <array>
#include <cmath>
#include <numeric>
#if defined(__ARM_NEON) || defined(__ARM_NEON__)
#include <arm_neon.h>
#endif
#include "../iprocessor.hpp"
#include "../../core/audio_buffer.hpp"

namespace Aura::DSP::Effects {

/**
 * @class TPTOnePole
 * @brief Topology Preserving Transform 1-Pole for air absorption.
 */
struct TPTOnePole {
    float s = 0.0f;
    float g = 0.5f;

    void setCutoff(float fc, float sr) {
        float wd = 2.0f * 3.14159265f * fc;
        float T = 1.0f / sr;
        float wa = (2.0f / T) * std::tan(wd * T / 2.0f);
        g = (wa * T / 2.0f) / (1.0f + (wa * T / 2.0f));
    }

    inline float processLP(float x) {
        float v = (x - s) * g;
        float y = v + s;
        s = y + v;
        return y;
    }
};

/**
 * @class ReverbCore
 * @brief High-quality algorithmic reverb.
 */
class ReverbCore : public IProcessor {
public:
    static constexpr size_t kNumLines = 8;
    
    ReverbCore(double sr = 44100.0) : m_sampleRate(sr) {
        const std::array<int, kNumLines> delaySamples = { 1031, 1153, 1321, 1459, 1601, 1823, 1999, 2333 };
        for (size_t i = 0; i < kNumLines; i++) {
            m_delays[i].assign(delaySamples[i] + 1, 0.0f);
            m_delayLengths[i] = delaySamples[i];
            m_readPos[i] = 0;
            m_damping[i].setCutoff(12000.0f, (float)sr); // Air absorption @ 12kHz
        }
        setDecay(2.4f);
    }

    void prepareToPlay(double sr, uint32_t /*bs*/) noexcept override {
        m_sampleRate = sr;
        for (auto& d : m_delays) std::fill(d.begin(), d.end(), 0.0f);
        for (auto& f : m_damping) f.setCutoff(12000.0f, (float)sr);
    }

    void reset() noexcept override {
        for (auto& d : m_delays) std::fill(d.begin(), d.end(), 0.0f);
    }

    std::string getName() const override { return "Aura Reverb Pro"; }

    void setDecay(float t60) {
        t60 = std::isfinite(t60) ? std::clamp(t60, 0.05f, 60.0f) : 2.4f;
        for (size_t i = 0; i < kNumLines; i++) {
            m_gains[i] = std::pow(10.0f, -3.0f * m_delayLengths[i] / (t60 * (float)m_sampleRate));
        }
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        for (uint32_t sample = 0; sample < buffer.getNumSamples(); ++sample) {
            const float inL = left[sample];
            const float inR = right[sample];
            const float input = 0.5f * (inL + inR);
            float wetL = 0.0f;
            float wetR = 0.0f;
            for (size_t line = 0; line < kNumLines; ++line) {
                auto& delay = m_delays[line];
                uint32_t& position = m_readPos[line];
                if (delay.empty()) continue;
                const float delayed = delay[position];
                const float filtered = m_damping[line].processLP(delayed);
                const float feedback = filtered * m_gains[line];
                delay[position] = input + feedback;
                position = (position + 1) % static_cast<uint32_t>(delay.size());
                const float polarity = (line & 1u) ? -1.0f : 1.0f;
                wetL += delayed * polarity;
                wetR += delayed * (line < 4 ? 1.0f : -1.0f);
            }
            constexpr float kNormalization = 0.18f;
            wetL *= kNormalization;
            wetR *= kNormalization;
            left[sample] = inL * (1.0f - m_mix) + wetL * m_mix;
            if (right != left) right[sample] = inR * (1.0f - m_mix) + wetR * m_mix;
        }
    }


private:
    double m_sampleRate;
    std::array<std::vector<float>, kNumLines> m_delays;
    std::array<uint32_t, kNumLines> m_delayLengths;
    std::array<uint32_t, kNumLines> m_readPos;
    std::array<float, kNumLines> m_gains;
    std::array<TPTOnePole, kNumLines> m_damping;
};

} // namespace Aura::DSP::Effects
