#pragma once

#include <vector>
#include <cmath>
#include <numbers>
#include "../analysis/analysis_engine.hpp"

namespace Hirari::Core::DSP::Mixing {

/**
 * @brief MasteringCore: High-fidelity master output processor.
 * Includes a brickwall limiter with sample-rate independent time constants.
 */
class MasteringCore {
public:
    explicit MasteringCore(double sampleRate) : m_sampleRate(sampleRate) {
        setTargetRelease(50.0); // Default 50ms release
    }

    void setTargetRelease(double releaseMs) {
        // PROFESSIONAL MATH: Convert release time (ms) to an exponential decay coefficient
        // T = -1 / (SampleRate * ln(TargetRatio))
        m_releaseCoeff = std::exp(-1.0 / (m_sampleRate * (releaseMs / 1000.0)));
    }

    /**
     * @brief Performs brickwall limiting and final loudness analysis.
     */
    void process(float* l, float* r, size_t numFrames) {
        for (size_t i = 0; i < numFrames; ++i) {
            float peak = std::max(std::abs(l[i]), std::abs(r[i]));
            
            // Peak detection and zero-overhead gain reduction
            if (peak > m_threshold) {
                m_gainReduction = m_threshold / peak;
            } else {
                // Smooth release back to 1.0 (Unity Gain)
                m_gainReduction = m_releaseCoeff * m_gainReduction + (1.0f - m_releaseCoeff) * 1.0f;
            }

            l[i] *= m_gainReduction;
            r[i] *= m_gainReduction;
        }

        // 2. PASS TO ANALYSIS
        Analysis::AnalysisEngine::getInstance().process(l, numFrames, m_sampleRate);
    }

private:
    double m_sampleRate;
    float m_threshold = 1.0f; // 0dBFS Brickwall
    float m_gainReduction = 1.0f;
    float m_releaseCoeff = 0.999f; // Dynamically calculated
};

} // namespace Hirari::Core::DSP::Mixing
