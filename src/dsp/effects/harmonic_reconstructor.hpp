#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include "../../core/audio_buffer.hpp"
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class HarmonicReconstructor
 * @brief High-frequency 'Air' restoration using non-linear projection.
 */
class HarmonicReconstructor : public IProcessor {
public:
    HarmonicReconstructor(double sr = 44100.0) : m_sampleRate(std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? static_cast<float>(sr) : 44'100.0f), m_cutoff(4500.0f) { updateCoefficients(); }

    std::string getName() const override { return "Harmonic Reconstructor"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }

    void prepareToPlay(double sr, uint32_t bs) noexcept override { (void)bs; m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? static_cast<float>(sr) : 44'100.0f; updateCoefficients(); reset(); }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi; (void)context;
        if (m_bypassed) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 32);
        for (uint32_t c = 0; c < channels; ++c) {
            float* data = buffer.getWritePointer(c);
            if (!data) continue;
            float low = m_lastIn[c];
            for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
                const float x = std::isfinite(data[i]) ? data[i] : 0.0f;
                low = m_alpha * low + (1.0f - m_alpha) * x;
                const float high = x - low;
                const float air = high * high * std::copysign(1.0f, high);
                const float y = x + 0.12f * air;
                data[i] = std::isfinite(y) ? std::clamp(y, -4.0f, 4.0f) : 0.0f;
            }
            m_lastIn[c] = low;
        }
    }

    void reset() noexcept override {
        std::fill(m_hpfState.begin(), m_hpfState.end(), 0.0f);
        std::fill(m_lastIn.begin(), m_lastIn.end(), 0.0f);
    }


private:
    void updateCoefficients() {
        float dt = 1.0f / m_sampleRate;
        float rc = 1.0f / (2.0f * M_PI * m_cutoff);
        m_alpha = rc / (rc + dt);
        m_hpfState.assign(32, 0.0f); // Support up to 32 channels
        m_lastIn.assign(32, 0.0f);
    }

    float m_cutoff;
    float m_sampleRate;
    float m_alpha = 0.9f;
    std::vector<float> m_hpfState;
    std::vector<float> m_lastIn;
};

} // namespace Aura::DSP::Effects
