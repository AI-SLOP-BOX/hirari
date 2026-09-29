#pragma once

#include <vector>
#include <cmath>
#include <algorithm>

namespace Hirari::DSP::Analysis {

/**
 * @brief DrumIsolator: Specialized Transient Separator for rhythmic extraction.
 * Uses a double-envelope tracking system to isolate drum hits from melodic material.
 */
class DrumIsolator {
public:
    explicit DrumIsolator(double sr) { setSampleRate(sr); }

    void setSampleRate(double sr) {
        m_sampleRate = sr;
        m_attackCoeff = static_cast<float>(1.0 - std::exp(-1.0 / (m_sampleRate * 0.001))); // 1ms
        m_releaseCoeff = static_cast<float>(1.0 - std::exp(-1.0 / (m_sampleRate * 0.1)));  // 100ms
    }

    /**
     * @brief ACCELERATED TRANSIENT SEPARATION.
     * HONEST FIX: Replaced expensive 'std::exp' per-sample with precomputed coefficients.
     */
    void process(const float* input, float* drumOutput, float* melodicOutput, size_t numFrames) {
        for (size_t i = 0; i < numFrames; ++i) {
            float env = std::abs(input[i]);
            
            // Double-envelope tracking
            float coeff = (env > m_envelope) ? m_attackCoeff : m_releaseCoeff;
            m_envelope += coeff * (env - m_envelope);

            // Ratio-based masking
            float mask = std::clamp((env / (m_envelope + 1e-6f)) * 0.5f, 0.0f, 1.0f);
            
            drumOutput[i] = input[i] * mask;
            melodicOutput[i] = input[i] * (1.0f - mask);
        }
    }

private:
    double m_sampleRate;
    float m_envelope = 0.0f;
    float m_attackCoeff = 0.01f;
    float m_releaseCoeff = 0.001f;
};

} // namespace Hirari::DSP::Analysis
