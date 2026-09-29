#pragma once

#include <vector>
#include <atomic>

namespace Hirari::Core::DSP::Mixing {

/**
 * @brief LatencyCompensationBuffer: Synchronizes all tracks for phase-perfect playback.
 * Compensates for the inherent delay of look-ahead effects like limiters.
 */
class LatencyCompensationBuffer {
public:
    explicit LatencyCompensationBuffer(size_t maxDelay) : m_maxDelaySamples(maxDelay) {
        m_buffer.resize(maxDelay, 0.0f);
    }

    /**
     * @brief Processes a sample through the delay line.
     * @param input: Raw sample.
     * @param currentLatency: Latency reported by the plugins on this track.
     * @param targetLatency: Total system max latency to align against.
     */
    float process(float input, size_t currentLatency, size_t targetLatency) {
        size_t delayNeeded = targetLatency - currentLatency;
        if (delayNeeded == 0) return input;

        m_buffer[m_writePos] = input;
        size_t readPos = (m_writePos + m_maxDelaySamples - delayNeeded) % m_maxDelaySamples;
        float output = m_buffer[readPos];
        
        m_writePos = (m_writePos + 1) % m_maxDelaySamples;
        return output;
    }

private:
    size_t m_maxDelaySamples;
    std::vector<float> m_buffer;
    size_t m_writePos = 0;
};

} // namespace Hirari::Core::DSP::Mixing
