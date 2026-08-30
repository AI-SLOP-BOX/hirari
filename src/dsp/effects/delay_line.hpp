#pragma once

#include <vector>
#include <atomic>
#include <algorithm>
#include <cmath>

namespace Aura::DSP::Effects {

/**
 * @brief DelayLine: Professional-grade sample-accurate delay.
 * Used for PDC (Plugin Delay Compensation) to align tracks in time.
 */
class DelayLine {
public:
    DelayLine(uint32_t maxDelaySamples) {
        // Enforce power-of-2 for fast bitwise masking
        m_mask = 1;
        while (m_mask < maxDelaySamples) m_mask <<= 1;
        m_buffer.assign(m_mask, 0.0f);
        m_mask -= 1;
    }

    /**
     * @brief Processes a single sample through the delay and returns the delayed value.
     */
    float process(float sample, uint32_t delaySamples) {
        if (m_buffer.empty()) return 0.0f;
        const float safeSample = std::isfinite(sample) ? sample : 0.0f;
        const uint32_t safeDelay = std::min(delaySamples, m_mask);
        m_buffer[m_writeIdx] = safeSample;
        const uint32_t readIdx = (m_writeIdx - safeDelay) & m_mask;
        const float output = m_buffer[readIdx];
        m_writeIdx = (m_writeIdx + 1) & m_mask;
        return std::isfinite(output) ? output : 0.0f;
    }


    void reset() {
        std::fill(m_buffer.begin(), m_buffer.end(), 0.0f);
        m_writeIdx = 0;
    }

private:
    std::vector<float> m_buffer;
    uint32_t m_writeIdx = 0;
    uint32_t m_mask = 0;
};

} // namespace Aura::DSP::Effects
