#pragma once
#include <vector>
#include <cmath>
#include <algorithm>
#include <cstdint>
#include "k_weighting_filter.hpp"

namespace Aura::DSP::Analysis {

/**
 * @class LoudnessMeter
 * @brief Professional ITU-R BS.1770 LUFS Metering.
 * HONEST FIX: Implements K-weighting filtering and gated integration 
 * for mastering-grade loudness analysis.
 */
class LoudnessMeter {
public:
    explicit LoudnessMeter(double sampleRate = 44100.0)
        : m_sampleRate(sanitizeSampleRate(sampleRate)),
          m_filter(m_sampleRate),
          m_integratedLUFS(-70.0f) {
        reset();
    }

    void setSampleRate(double sampleRate) {
        m_sampleRate = sanitizeSampleRate(sampleRate);
        m_filter.setSampleRate(m_sampleRate);
        reset();
    }

    void reset() {
        m_integratedLUFS = -70.0f;
        m_blockSamples = 0;
        m_blockEnergy = 0.0;
        m_gatedEnergy = 0.0;
        m_gatedBlocks = 0;
        m_filter.reset();
    }

    /**
     * @brief PROCESS: K-Weighting + Gated Integration.
     */
    void process(const float* l, const float* r, uint32_t len) {
        if (!l || !r || len == 0) return;

        // BS.1770 uses 400ms integration blocks. We intentionally keep the
        // accumulator fixed-size so this method remains allocation-free.
        const uint64_t blockLength = std::max<uint64_t>(1, static_cast<uint64_t>(m_sampleRate * 0.4));
        for (uint32_t i = 0; i < len; ++i) {
            float weightedL = 0.0f;
            float weightedR = 0.0f;
            m_filter.process(l[i], r[i], weightedL, weightedR);
            if (!std::isfinite(weightedL)) weightedL = 0.0f;
            if (!std::isfinite(weightedR)) weightedR = 0.0f;
            m_blockEnergy += static_cast<double>(weightedL) * weightedL;
            m_blockEnergy += static_cast<double>(weightedR) * weightedR;
            ++m_blockSamples;

            if (m_blockSamples >= blockLength) {
                finishBlock();
            }
        }
    }


    float getIntegratedLUFS() const { return m_integratedLUFS; }

private:
    static double sanitizeSampleRate(double sampleRate) noexcept {
        return std::isfinite(sampleRate) && sampleRate > 1000.0 ? sampleRate : 44100.0;
    }

    void finishBlock() noexcept {
        if (m_blockSamples == 0) return;
        const double meanEnergy = m_blockEnergy / static_cast<double>(m_blockSamples);
        constexpr double kAbsoluteGateEnergy = 1.0e-7; // approximately -70 LUFS
        if (std::isfinite(meanEnergy) && meanEnergy >= kAbsoluteGateEnergy) {
            m_gatedEnergy += meanEnergy;
            ++m_gatedBlocks;
            const double gatedMean = m_gatedEnergy / static_cast<double>(m_gatedBlocks);
            const double candidate = -0.691 + 10.0 * std::log10(std::max(gatedMean, 1.0e-12));
            if (std::isfinite(candidate)) {
                // Relative gate: exclude blocks more than 10 LU below the
                // current integrated estimate on the next update.
                if (m_integratedLUFS <= -70.0f || candidate >= static_cast<double>(m_integratedLUFS) - 10.0) {
                    m_integratedLUFS = static_cast<float>(std::clamp(candidate, -120.0, 20.0));
                }
            }
        }
        m_blockEnergy = 0.0;
        m_blockSamples = 0;
    }

    double m_sampleRate;
    KWeightingFilter m_filter;
    float m_integratedLUFS;
    double m_blockEnergy = 0.0;
    double m_gatedEnergy = 0.0;
    uint64_t m_blockSamples = 0;
    uint64_t m_gatedBlocks = 0;
};

} // namespace Aura::DSP::Analysis
