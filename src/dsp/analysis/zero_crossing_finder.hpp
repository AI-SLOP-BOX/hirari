#pragma once

#include <cmath>
#include <cstdint>
#include <algorithm>
#include <limits>

namespace Aura::DSP::Analysis {

/**
 * @brief ZeroCrossingFinder: Precision audio editing utility.
 * Locates the nearest sample where the waveform crosses 0dB to prevent "clicks" during cuts.
 */
class ZeroCrossingFinder {
public:
    /**
     * @brief Finds the nearest zero-crossing point around the target position.
     * @param data: Audio buffer data.
     * @param targetPos: Desired edit point.
     * @param searchRange: Number of samples to look in either direction.
     * @return The optimal sample index for a clean cut.
     */
    static size_t findNearest(const float* data, size_t targetPos, size_t numSamples, size_t searchRange = 128) {
        if (!data || numSamples == 0) return 0;
        targetPos = std::min(targetPos, numSamples - 1u);
        const size_t begin = targetPos > searchRange ? targetPos - searchRange : 0;
        const size_t end = std::min(numSamples - 1u, targetPos + searchRange);
        size_t best = targetPos;
        float bestScore = std::numeric_limits<float>::max();
        for (size_t i = begin; i <= end; ++i) {
            const float current = std::isfinite(data[i]) ? data[i] : 0.0f;
            const float next = i + 1u < numSamples && std::isfinite(data[i + 1u]) ? data[i + 1u] : current;
            const bool crossing = (current <= 0.0f && next >= 0.0f) || (current >= 0.0f && next <= 0.0f);
            const float score = crossing ? std::fabs(current) + 0.05f * static_cast<float>(std::abs(static_cast<int64_t>(i) - static_cast<int64_t>(targetPos)))
                                         : 1000.0f + std::fabs(current);
            if (score < bestScore) { bestScore = score; best = i; }
        }
        return best;
    }

};

} // namespace Aura::DSP::Analysis
