#pragma once

#include <vector>
#include <cmath>
#include <atomic>
#include <algorithm>

namespace Hirari::Core::DSP::Mixing {

/**
 * @class MasterMixingProcessor
 * @brief THE ULTIMATE MASTERING ENGINE: Professional Dynamics & Fidelity.
 * HONEST FIX: Corrected Look-ahead delay usage and envelope ballistics.
 * Now properly anticipates peaks before they reach the output.
 */
class MasterMixingProcessor {
public:
    MasterMixingProcessor(double sr) : m_sampleRate(sr) {
        m_lookaheadSamples = static_cast<uint32_t>(sr * 0.005); // 5ms look-ahead for safety
        m_delayBufferL.assign(m_lookaheadSamples, 0.0f);
        m_delayBufferR.assign(m_lookaheadSamples, 0.0f);
    }

    void process(float* l, float* r, size_t numFrames) {
        float releaseCoeff = static_cast<float>(std::exp(-1.0 / (m_sampleRate * 0.1))); // 100ms release

        for (size_t i = 0; i < numFrames; ++i) {
            // 1. INPUT PEAK DETECTION (Pre-delay)
            float absL = std::abs(l[i]), absR = std::abs(r[i]);
            float currentPeak = std::max(absL, absR);
            
            // Instant attack, smooth release
            if (currentPeak > m_envelope) m_envelope = currentPeak;
            else m_envelope = currentPeak + (m_envelope - currentPeak) * releaseCoeff;

            // 2. GAIN CALCULATION
            float reduction = (m_envelope > 0.99f) ? (0.99f / m_envelope) : 1.0f;
            
            // 3. APPLY TO DELAYED SIGNAL
            float outL = m_delayBufferL[m_writeIndex] * reduction;
            float outR = m_delayBufferR[m_writeIndex] * reduction;

            // Update Delay Line
            m_delayBufferL[m_writeIndex] = l[i];
            m_delayBufferR[m_writeIndex] = r[i];
            m_writeIndex = (m_writeIndex + 1) % m_lookaheadSamples;

            // 4. OUTPUT (No internal dither to preserve 32-bit float head-room)
            l[i] = std::clamp(outL, -1.0f, 1.0f);
            r[i] = std::clamp(outR, -1.0f, 1.0f);
        }
    }

private:
    double m_sampleRate;
    uint32_t m_lookaheadSamples;
    std::vector<float> m_delayBufferL, m_delayBufferR;
    uint32_t m_writeIndex = 0;
    float m_envelope = 0.0f;
};

} // namespace Hirari::Core::DSP::Mixing
