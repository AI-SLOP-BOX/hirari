#pragma once

#include <vector>
#include <atomic>
#include "../../core/audio_buffer.hpp"

namespace Aura::DSP::Effects {

/**
 * @brief SidechainLink: Enables inter-track dynamic routing.
 * Addresses the "missing sidechain infrastructure" from the review.
 */
class SidechainLink {
public:
    struct Envelope {
        float level = 0.0f;
    };

    /**
     * @brief Updates the sidechain level from a source track.
     */
    void updateFromSource(const Core::AudioBuffer& buffer) {
        const uint32_t channels = buffer.getNumChannels();
        const uint32_t samples = buffer.getNumSamples();
        if (channels == 0 || samples == 0) { m_level.store(0.0f, std::memory_order_relaxed); return; }
        double sum = 0.0;
        for (uint32_t c = 0; c < channels; ++c) {
            const float* p = buffer.getReadPointer(c);
            if (!p) continue;
            for (uint32_t i = 0; i < samples; ++i) {
                const float x = std::isfinite(p[i]) ? p[i] : 0.0f;
                sum += static_cast<double>(x) * x;
            }
        }
        const float rms = static_cast<float>(std::sqrt(sum / static_cast<double>(channels * samples)));
        const float previous = m_level.load(std::memory_order_relaxed);
        const float target = std::isfinite(rms) ? rms : 0.0f;
        const float smoothed = previous + (target - previous) * 0.25f;
        m_level.store(std::clamp(smoothed, 0.0f, 4.0f), std::memory_order_relaxed);
    }


    float getLevel() const { return m_level.load(); }

private:
    std::atomic<float> m_level{0.0f};
};

} // namespace Aura::DSP::Effects
