#pragma once

#include <cmath>
#include <algorithm>
#include <vector>
#include "../math/fast_math.hpp"

namespace Hirari::DSP::Analysis {

/**
 * @brief PhaseCorrelator: Monitors L/R phase relationship.
 * Essential for Mono compatibility.
 * Values: +1 (Mono/In-phase), 0 (Wide/Decorrelated), -1 (Anti-phase/Destructive).
 */
class PhaseCorrelator {
public:
    PhaseCorrelator(double sr = 44100.0) : m_sampleRate(sr) {
        m_historySize = static_cast<size_t>(sr * 0.05); // 50ms integration window
    }

    /**
     * @brief Calculates the correlation coefficient for a block.
     * @return float Correlation [-1.0, 1.0]
     */
    float analyze(const float* l, const float* r, uint32_t numSamples) {
        double sumLR = 0.0, sumLL = 0.0, sumRR = 0.0;

        for (uint32_t i = 0; i < numSamples; ++i) {
            sumLR += static_cast<double>(l[i] * r[i]);
            sumLL += static_cast<double>(l[i] * l[i]);
            sumRR += static_cast<double>(r[i] * r[i]);
        }

        double norm = std::sqrt(sumLL * sumRR);
        if (norm < 1e-12) return 1.0f; // Silence is correlated

        float instantCorr = static_cast<float>(sumLR / norm);
        
        // Exponential Smoothing for the meter response
        m_smoothedCorr += (instantCorr - m_smoothedCorr) * 0.1f;
        return std::clamp(m_smoothedCorr, -1.0f, 1.0f);
    }

private:
    double m_sampleRate;
    size_t m_historySize;
    float m_smoothedCorr = 0.0f;
};

} // namespace Hirari::DSP::Analysis
