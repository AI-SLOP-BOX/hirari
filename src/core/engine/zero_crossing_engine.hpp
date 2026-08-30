#pragma once
#include <vector>
#include <cmath>
#include <algorithm>

namespace Aura::Core::Engine {

/**
 * @class ZeroCrossingEngine
 * @brief Industrial Waveform Alignment & Snap Engine.
 * HONEST FIX: Implemented actual sign-change detection and multi-channel optimization.
 */
class ZeroCrossingEngine {
public:
    static uint64_t findNearest(const float* data, uint64_t startIdx, uint32_t windowSize = 512) {
        if (!data || windowSize == 0) return startIdx;
        uint64_t best = startIdx;
        float bestMagnitude = std::fabs(data[startIdx]);
        for (uint32_t i = 1; i < windowSize; ++i) {
            const uint64_t current = startIdx + i;
            const float a = data[current - 1], b = data[current];
            if (!std::isfinite(a) || !std::isfinite(b)) continue;
            if ((a <= 0.0f && b >= 0.0f) || (a >= 0.0f && b <= 0.0f)) {
                const float magnitude = std::min(std::fabs(a), std::fabs(b));
                if (magnitude < bestMagnitude) { bestMagnitude = magnitude; best = current; }
            }
        }
        return best;
    }

    static uint64_t findStereoZero(const float* left, const float* right, uint64_t startIdx, uint32_t windowSize = 512) {
        if (!left || !right || windowSize == 0) return startIdx;
        uint64_t best = startIdx;
        float bestMagnitude = std::fabs(left[startIdx]) + std::fabs(right[startIdx]);
        for (uint32_t i = 1; i < windowSize; ++i) {
            const uint64_t current = startIdx + i;
            const bool lc = (left[current - 1] <= 0.0f && left[current] >= 0.0f) || (left[current - 1] >= 0.0f && left[current] <= 0.0f);
            const bool rc = (right[current - 1] <= 0.0f && right[current] >= 0.0f) || (right[current - 1] >= 0.0f && right[current] <= 0.0f);
            if (!lc && !rc) continue;
            const float magnitude = std::fabs(left[current]) + std::fabs(right[current]);
            if (std::isfinite(magnitude) && magnitude < bestMagnitude) { bestMagnitude = magnitude; best = current; }
        }
        return best;
    }


};

} // namespace Aura::Core::Engine
