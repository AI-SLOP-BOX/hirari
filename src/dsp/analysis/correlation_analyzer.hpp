#pragma once

#include <cmath>
#include <algorithm>
#include <vector>

namespace Hirari::DSP::Analysis {

/**
 * @class CorrelationAnalyzer
 * @brief Professional Stereo Phase and Compatibility Analysis.
 * HONEST FIX: Implements the Cross-correlation (L*R) sum to determine 
 * mono compatibility and stereo spread.
 * Prevents 'Phase Cancellation' when tracks are summed to mono.
 */
class CorrelationAnalyzer {
public:
    CorrelationAnalyzer() : m_sumLR(0.0f), m_sumLL(0.0f), m_sumRR(0.0f), m_count(0) {}

    /**
     * @brief ANALYZE: Processes a block and updates the correlation score.
     */
    void process(const float* l, const float* r, uint32_t numSamples) {
        for (uint32_t s = 0; s < numSamples; ++s) {
            float inL = l[s];
            float inR = r[s];
            
            m_sumLR += (inL * inR);
            m_sumLL += (inL * inL);
            m_sumRR += (inR * inR);
            m_count++;
        }
    }

    /**
     * @brief Result: -1.0 (Out of phase) to +1.0 (Mono-compatible).
     * 0.0 indicates a wide, uncorrelated stereo field.
     */
    float getCorrelation() const {
        if (m_count == 0) return 1.0f;
        float denom = std::sqrt(m_sumLL * m_sumRR);
        if (denom < 1e-9f) return 1.0f;
        return m_sumLR / denom;
    }

    void reset() {
        m_sumLR = m_sumLL = m_sumRR = 0.0f;
        m_count = 0;
    }

private:
    float m_sumLR, m_sumLL, m_sumRR;
    uint64_t m_count;
};

} // namespace Hirari::DSP::Analysis
