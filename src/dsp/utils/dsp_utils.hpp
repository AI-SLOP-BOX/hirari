#pragma once

#include <cmath>
#include <algorithm>
#include <cstdint>

namespace Hirari::DSP::Utils {

/**
 * @brief DSP Utils: High-performance bitwise and mathematical helpers.
 * Addresses the "fragmented constants" and "duplicate logic" from the review.
 */
class DSPUtils {
public:
    static constexpr size_t kMaxBufferSize = 1024;
    static constexpr uint32_t kIndexMask = kMaxBufferSize - 1;

    /**
     * @brief Constants
     */
    static constexpr float PI = 3.14159265358979323846f;
    static constexpr float TWO_PI = 6.28318530717958647692f;
    static constexpr float HALF_PI = 1.57079632679489661923f;

    /**
     * @brief High-speed hard clipper.
     */
    static inline float hardClip(float in) {
        return std::clamp(in, -1.0f, 1.0f);
    }

    /**
     * @brief Sample-rate independent decay coefficient calculation.
     */
    static inline float calculateDecayCoeff(float releaseMs, double sampleRate) {
        return std::exp(-1.0f / (releaseMs * 0.001f * static_cast<float>(sampleRate)));
    }

    /**
     * @brief DB to linear gain conversion.
     */
    static inline float dbToLinear(float db) {
        return std::pow(10.0f, db / 20.0f);
    }

    /**
     * @brief PROFESSIONAL HERMITE CUBIC (4-Point)
     * Unified interpolation used to replace duplicate code in sources and regions.
     */
    static inline float interpolateHermite(float y0, float y1, float y2, float y3, float t) {
        const float a = (-0.5f * y0 + 1.5f * y1 - 1.5f * y2 + 0.5f * y3);
        const float b = (y0 - 2.5f * y1 + 2.0f * y2 - 0.5f * y3);
        const float c = (-0.5f * y0 + 0.5f * y2);
        const float d = y1;
        return a * t * t * t + b * t * t + c * t + d;
    }

    /**
     * @brief NAN/INF PROTECTOR (Safety Guard)
     * Clears calculated values that are outside the range of audio signals.
     */
    static inline float sanitize(float f) {
        union { float f; uint32_t i; } u = { f };
        if ((u.i & 0x7f800000) == 0x7f800000) return 0.0f;
        return f;
    }
};


} // namespace Hirari::DSP::Utils
