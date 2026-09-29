#pragma once

#include <cmath>

namespace Hirari::DSP::Math {

/**
 * @brief Interpolators: High-fidelity sample reconstruction utilities.
 * Addresses "insufficient quality" and "low efficiency" from the professional review.
 */
class Interpolators {
public:
    /**
     * @brief Linear Interpolation (Fast for non-critical signals).
     */
    static inline float linear(const float* data, float f) {
        return data[0] + f * (data[1] - data[0]);
    }

    /**
     * @brief Cubic Hermite Spline Interpolation (Pro Audio Standard).
     * High-fidelity 4-point reconstruction.
     */
    static inline float cubic(const float* s, float f) {
        const float f2 = f * f;
        const float f3 = f2 * f;

        const float a = (-0.5f * s[-1] + 1.5f * s[0] - 1.5f * s[1] + 0.5f * s[2]);
        const float b = (s[-1] - 2.5f * s[0] + 2.0f * s[1] - 0.5f * s[2]);
        const float c = (-0.5f * s[-1] + 0.5f * s[1]);
        const float d = s[0];

        return ((a * f3) + (b * f2) + (c * f) + d);
    }
};

} // namespace Hirari::DSP::Math
