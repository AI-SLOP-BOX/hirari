#pragma once

#include <vector>
#include <array>
#include <cmath>
#include <numeric>
#include <limits>
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
        sr = (std::isfinite(sr) && sr > 100.0f) ? sr : 44100.0f;
        fc = std::clamp(std::isfinite(fc) ? fc : 12000.0f, 5.0f, sr * 0.45f);
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
    
    ReverbCore(double sr = 44100.0) : m_sampleRate(
        std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44'100.0) {
        setupDelays();
        setDecay(2.4f);
    }

    void prepareToPlay(double sr, uint32_t /*bs*/) noexcept override {
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0
            ? sr
            : 44'100.0;
        setupDelays();
        for (auto& f : m_damping) f.setCutoff(12000.0f, static_cast<float>(m_sampleRate));
        setDecay(m_decaySeconds);
    }

    void reset() noexcept override {
        for (auto& d : m_delays) std::fill(d.begin(), d.end(), 0.0f);
    }

    // Report the amount of silence the renderer must append after the last
    // input sample so a 60 dB decay is not truncated during bounce/export.
    uint32_t getTailSamples() const noexcept override {
        const double rate = std::isfinite(m_sampleRate) && m_sampleRate > 0.0
            ? m_sampleRate
            : 44'100.0;
        const double decay = std::isfinite(m_decaySeconds)
            ? std::clamp(static_cast<double>(m_decaySeconds), 0.05, 60.0)
            : 2.4;
        double maxDelay = 0.0;
        for (const auto delay : m_delayLengths) maxDelay = std::max(maxDelay, static_cast<double>(delay));
        const double samples = maxDelay + (3.0 * decay * rate);
        return static_cast<uint32_t>(std::min<double>(
            static_cast<double>(std::numeric_limits<uint32_t>::max()),
            std::ceil(std::max(0.0, samples))));
    }

    std::string getName() const override { return "Aura Reverb Pro"; }

    void setDecay(float t60) {
        t60 = std::isfinite(t60) ? std::clamp(t60, 0.05f, 60.0f) : 2.4f;
        m_decaySeconds = t60;
        for (size_t i = 0; i < kNumLines; i++) {
            m_gains[i] = std::pow(10.0f, -3.0f * m_delayLengths[i] / (t60 * (float)m_sampleRate));
        }
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        if (!left || !right) return;
        const float mix = std::isfinite(m_mix) ? std::clamp(m_mix, 0.0f, 1.0f) : 0.0f;
        for (uint32_t sample = 0; sample < buffer.getNumSamples(); ++sample) {
            const float inL = std::isfinite(left[sample]) ? left[sample] : 0.0f;
            const float inR = std::isfinite(right[sample]) ? right[sample] : 0.0f;
            const float input = 0.5f * (inL + inR);
            float wetL = 0.0f;
            float wetR = 0.0f;
            for (size_t line = 0; line < kNumLines; ++line) {
                auto& delay = m_delays[line];
                uint32_t& position = m_readPos[line];
                if (delay.empty()) continue;
                const float delayed = delay[position];
                const float filtered = m_damping[line].processLP(delayed);
                const float feedback = std::isfinite(filtered * m_gains[line])
                    ? filtered * m_gains[line]
                    : 0.0f;
                delay[position] = input + feedback;
                position = (position + 1) % static_cast<uint32_t>(delay.size());
                const float polarity = (line & 1u) ? -1.0f : 1.0f;
                wetL += delayed * polarity;
                wetR += delayed * (line < 4 ? 1.0f : -1.0f);
            }
            constexpr float kNormalization = 0.18f;
            wetL *= kNormalization;
            wetR *= kNormalization;
            left[sample] = (inL * (1.0f - mix) + wetL * mix);
            if (right != left) right[sample] = (inR * (1.0f - mix) + wetR * mix);
            if (!std::isfinite(left[sample])) left[sample] = 0.0f;
            if (right != left && !std::isfinite(right[sample])) right[sample] = 0.0f;
        }
    }


private:
    void setupDelays() {
        constexpr std::array<uint32_t, kNumLines> base = {1031u, 1153u, 1321u, 1459u,
                                                          1601u, 1823u, 1999u, 2333u};
        const double scale = m_sampleRate / 44'100.0;
        for (size_t i = 0; i < kNumLines; ++i) {
            const uint32_t length = std::max<uint32_t>(1u,
                static_cast<uint32_t>(std::llround(static_cast<double>(base[i]) * scale)));
            m_delays[i].assign(static_cast<size_t>(length) + 1u, 0.0f);
            m_delayLengths[i] = length;
            m_readPos[i] = 0;
            m_damping[i].setCutoff(12000.0f, static_cast<float>(m_sampleRate));
        }
    }

    double m_sampleRate;
    std::array<std::vector<float>, kNumLines> m_delays;
    std::array<uint32_t, kNumLines> m_delayLengths;
    std::array<uint32_t, kNumLines> m_readPos;
    std::array<float, kNumLines> m_gains;
    std::array<TPTOnePole, kNumLines> m_damping;
    float m_decaySeconds = 2.4f;
};

} // namespace Aura::DSP::Effects
