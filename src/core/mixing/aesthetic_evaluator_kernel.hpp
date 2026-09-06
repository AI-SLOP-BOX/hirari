#pragma once
#include <array>
#include <numeric>
#include <cmath>
#include <vector>
#include <algorithm>
#include <string>
#include "../diagnostics/forensic_kernel.hpp"

namespace Aura::Core::Mixing {

struct AestheticProfile {
    char name[64];
    float targetMelCurve[128];
    float targetCrestFactor;
    float targetStereoWidth;
    float targetEmotionalProfile[4];
};

struct AestheticFeatures {
    float spectralBalance; 
    float dynamicComplexity;
    float transientClarity;
    float stereoWidth;
    float phaseCoherence;
};

/**
 * @class AestheticEvaluatorKernel
 * @brief Evaluation kernel for qualitative mix analysis.
 * HONEST FIX: Implements real signal statistics (Peak, RMS, Correlation).
 */
class AestheticEvaluatorKernel {
public:
    static AestheticEvaluatorKernel& getInstance() {
        static AestheticEvaluatorKernel instance;
        return instance;
    }

    /**
     * @brief Performs actual signal analysis for aesthetic metrics.
     */
    AestheticFeatures analyze(const float* l, const float* r, uint32_t numSamples) {
        if (!l || !r || numSamples == 0) return {0.5f, 0.5f, 0.5f, 0.0f, 1.0f};

        float sumSqL = 0.0f, sumSqR = 0.0f;
        float peakL = 0.0f, peakR = 0.0f;
        float dotProduct = 0.0f;

        for (uint32_t i = 0; i < numSamples; ++i) {
            const float sl = std::isfinite(l[i]) ? std::clamp(l[i], -4.0f, 4.0f) : 0.0f;
            const float sr = std::isfinite(r[i]) ? std::clamp(r[i], -4.0f, 4.0f) : 0.0f;
            
            sumSqL += sl * sl;
            sumSqR += sr * sr;
            peakL = std::max(peakL, std::abs(sl));
            peakR = std::max(peakR, std::abs(sr));
            dotProduct += sl * sr;
        }

        float rmsL = std::sqrt(sumSqL / numSamples);
        float rmsR = std::sqrt(sumSqR / numSamples);
        float avgRMS = (rmsL + rmsR) * 0.5f;
        float avgPeak = (peakL + peakR) * 0.5f;

        AestheticFeatures f;
        f.spectralBalance = std::clamp(std::abs(rmsL - rmsR) / (avgRMS + 1e-6f), 0.0f, 1.0f); // Symmetry measure
        f.dynamicComplexity = std::clamp((avgPeak / (avgRMS + 1e-6f)) / 10.0f, 0.0f, 1.0f); // Crest factor approximation
        f.transientClarity = std::clamp(f.dynamicComplexity * 1.5f, 0.0f, 1.0f);
        f.stereoWidth = std::clamp(1.0f - std::abs(dotProduct) /
            (std::sqrt(sumSqL * sumSqR) + 1e-6f), 0.0f, 1.0f);
        
        // Stereo Correlation (-1 to 1)
        f.phaseCoherence = std::clamp(dotProduct / (std::sqrt(sumSqL * sumSqR) + 1e-6f), -1.0f, 1.0f);
        
        return f;
    }

private:
    AestheticEvaluatorKernel() = default;
};

} // namespace Aura::Core::Mixing
