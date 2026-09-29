#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include "loudness_meter.hpp"

namespace Hirari::DSP::Analysis {

/**
 * @brief LoudnessNormalizer: Professional Mastering Output normalization.
 * Automatically aligns your track to streaming standards (-14 LUFS, etc.).
 */
class LoudnessNormalizer {
public:
    static float calculateTargetGain(const float* l, const float* r, uint64_t len, 
                                     double sr, float targetLUFS = -14.0f) {
        LoudnessMeter meter(sr);
        
        // 1. ANALYZE ENTIRE BUFFER
        uint32_t step = 1024;
        for (uint64_t i = 0; i < len; i += step) {
            uint32_t currentLen = std::min(step, static_cast<uint32_t>(len - i));
            meter.process(l + i, r + i, currentLen);
        }

        // 2. CALCULATE DELTA
        float currentLUFS = meter.getMetrics().integratedLUFS;
        float dbDiff = targetLUFS - currentLUFS;

        // 3. RETURN MULTIPLIER (Gain)
        return std::pow(10.0f, dbDiff / 20.0f);
    }
};

} // namespace Hirari::DSP::Analysis
