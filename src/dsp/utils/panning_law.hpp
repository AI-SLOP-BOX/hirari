#pragma once

#include <cmath>
#include <algorithm>

namespace Hirari::DSP::Mixing {

/**
 * @brief PanningLaw: Constant Power (-3dB) Panning for precise stereo imaging.
 * Addresses the "unnatural volume shifts when panning" identifier from the review.
 */
class PanningLaw {
public:
    static constexpr float kPiDiv4 = 0.785398163f;

    /**
     * @brief Calculates L/R gains based on constant power law.
     * @param pan: -1.0 (Full Left) to 1.0 (Full Right)
     */
    static inline void calculate(float pan, float& gainL, float& gainR) {
        float p = (pan + 1.0f) * 0.5f; // Normalize to 0.0 - 1.0
        p = std::clamp(p, 0.0f, 1.0f);

        // Sin/Cos Panning for Constant Power
        gainL = std::cos(p * kPiDiv4 * 2.0f);
        gainR = std::sin(p * kPiDiv4 * 2.0f);
    }
};

} // namespace Hirari::DSP::Mixing
