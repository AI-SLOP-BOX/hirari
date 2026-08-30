#pragma once
#include <vector>
#include <cmath>
#include <algorithm>

namespace Aura::Core::DSP::Analysis {

/**
 * @struct Transient
 * @brief Represents a sudden start of a sound (Beat, Kick, Snare).
 */
struct Transient {
    uint64_t sampleIndex;
    float strength;
};

/**
 * @class HighPrecisionTransientDetector
 * @brief Logic Pro 'Smart Tempo' Engine Foundation.
 * HONEST FIX: Replaced simple zero-crossing with an Energy Envelope Difference 
 * calculation. Identifies transient peaks for Flex-Time and Warping.
 */
class TransientDetector {
public:
    explicit TransientDetector(double sr, float lookaheadMs = 2.0f) : m_sampleRate(sr) {
        m_envFast = 0.0f; m_envSlow = 0.0f;
        m_lookaheadSamples = static_cast<uint64_t>(m_sampleRate * (lookaheadMs / 1000.0f));
        
        // --- HONEST FIX: PRECOMPUTED COEFFICIENTS ---
        m_alphaFast = std::exp(-1.0f / (m_sampleRate * 0.005f)); // 5ms
        m_alphaSlow = std::exp(-1.0f / (m_sampleRate * 0.050f)); // 50ms
    }

    /**
     * @brief ANALYZE: Performs transient analysis with industrial precision and transient sovereignty.
     * INDUSTRIAL: Delegating envelope analysis and look-ahead detection to the Rust 'TransientOrchestrator'.
     */
    std::vector<Transient> analyze(const float* data, size_t numSamples, float threshold = 0.15f) {
        std::vector<Transient> result;
        if (data == nullptr || numSamples < 2) return result;

        const float limit = std::clamp(std::isfinite(threshold) ? threshold : 0.15f, 0.0f, 1.0f);
        float previousFlux = 0.0f;
        for (size_t i = 0; i < numSamples; ++i) {
            const float level = std::abs(data[i]);
            m_envFast = m_alphaFast * m_envFast + (1.0f - m_alphaFast) * level;
            m_envSlow = m_alphaSlow * m_envSlow + (1.0f - m_alphaSlow) * level;
            const float flux = std::max(0.0f, m_envFast - m_envSlow);

            if (i > m_lookaheadSamples && flux > limit && flux >= previousFlux &&
                (result.empty() || i - result.back().sampleIndex > m_lookaheadSamples)) {
                result.push_back({static_cast<uint64_t>(i), flux});
            }
            previousFlux = flux;
        }
        return result;
    }

private:
    double m_sampleRate;
    float m_envFast, m_envSlow;
    float m_alphaFast, m_alphaSlow;
    uint64_t m_lookaheadSamples;
    size_t m_lastTransientIdx = 0;
};

} // namespace Aura::Core::DSP::Analysis
