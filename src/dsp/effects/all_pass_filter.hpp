#pragma once

#include <vector>
#include <cmath>

namespace Aura::DSP::Effects {

/**
 * @brief AllPassFilter: Phase-shifting without amplitude change for diffusion.
 * Addresses the "low echo density" concern in the professional review.
 */
class AllPassFilter {
public:
    AllPassFilter(size_t delaySamples, float feedback) 
        : m_feedback(feedback) {
        uint32_t size = 1;
        while (size <= (uint32_t)delaySamples) size <<= 1;
        m_delayBuffer.resize(size, 0.0f);
        m_mask = size - 1;
    }

    float process(float in) {
        if (m_delayBuffer.empty()) return in;
        const float safeIn = std::isfinite(in) ? in : 0.0f;
        const float delayed = m_delayBuffer[m_idx];
        const float output = delayed - m_feedback * safeIn;
        m_delayBuffer[m_idx] = safeIn + m_feedback * delayed;
        m_idx = (m_idx + 1u) & m_mask;
        return std::isfinite(output) ? output : 0.0f;
    }


private:
    std::vector<float> m_delayBuffer;
    float m_feedback;
    size_t m_idx = 0;
    uint32_t m_mask;
};

} // namespace Aura::DSP::Effects
