#pragma once

#include <vector>
#include <cmath>

namespace Hirari::DSP::Mixing {

/**
 * @brief LookaheadDelay: Independent high-performance delay for future-peeking.
 * Essential logic for True-Peak Limiting and Intelligent Compression.
 */
template <size_t MaxDelay = 4096>
class LookaheadDelay {
public:
    static_assert((MaxDelay & (MaxDelay - 1)) == 0, "MaxDelay must be power of 2");

    LookaheadDelay() : m_writeIdx(0) {
        m_buffer.resize(MaxDelay, 0.0f);
    }

    /**
     * @brief Pushes a new sample and returns the delayed (look-ahead) sample.
     */
    float process(float in, size_t delayFrames) {
        m_buffer[m_writeIdx & (MaxDelay - 1)] = in;
        float out = m_buffer[(m_writeIdx - delayFrames) & (MaxDelay - 1)];
        m_writeIdx++;
        return out;
    }

private:
    std::vector<float> m_buffer;
    size_t m_writeIdx;
};

} // namespace Hirari::DSP::Mixing
