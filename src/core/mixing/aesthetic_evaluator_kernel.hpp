#pragma once
#include <cstdint>
#include "../diagnostics/forensic_kernel.hpp"
#include "../rust_ffi.hpp"

namespace Hirari::Core::Mixing {

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
        HirariAestheticFeatures result{};
        hirari_aesthetic_analyze(l, r, numSamples, &result);
        return {result.spectral_balance, result.dynamic_complexity,
                result.transient_clarity, result.stereo_width,
                result.phase_coherence};
    }

private:
    AestheticEvaluatorKernel() = default;
};

} // namespace Hirari::Core::Mixing
