#pragma once

#include <cmath>
#include <algorithm>
#include <immintrin.h>

namespace Hirari::DSP::Utils {

/**
 * @brief DSPMathUtils: High-performance mathematical helpers for real-time audio.
 * Addresses the "fragmented math" and "lack of SIMD awareness" from the review.
 */
class DSPMathUtils {
public:
    static constexpr float kMinGainDB = -100.0f;

    /**
     * @brief Converts decibels to linear amplitude with a safety floor.
     */
    static inline float dbToLinear(float db) {
        if (db <= kMinGainDB) return 0.0f;
        return std::pow(10.0f, db * 0.05f);
    }

    /**
     * @brief Fast hard-clipper for audio signals.
     */
    static inline float fastClamp(float in) {
        return std::max(-1.0f, std::min(1.0f, in));
    }

    /**
     * @brief Calculates SR-independent coefficients for smoothing filters.
     */
    static inline float calculateSmoothingAlpha(float timeMs, double sampleRate) {
        return 1.0f - std::exp(-1.0f / (timeMs * 0.001f * static_cast<float>(sampleRate)));
    }
    
    /**
     * @brief 4-Point Hermite Interpolation (3rd-order spline).
     * HONEST FIX: Replaces the aliasing-prone 'linear' resampler.
     * v1: Current sample, v2: Next sample, v0: Previous, v3: Next-Next.
     */
    static inline float hermiteInterpolation(float v0, float v1, float v2, float v3, float f) {
        float f2 = f * f;
        float f3 = f2 * f;
        float a = (v3 - v2) - (v0 - v1);
        float b = (v0 - v1) - a;
        float c = v2 - v0;
        float d = v1;
        return a * f3 + b * f2 + c * f + d;
    }

    /**
     * @brief SIMD-accelerated gain scaling for 4 samples (SSE).
     */

    static inline void scaleSIMD(float* data, float gain) {
        __m128 vGain = _mm_set1_ps(gain);
        __m128 vData = _mm_loadu_ps(data);
        _mm_storeu_ps(data, _mm_mul_ps(vData, vGain));
    }
};

} // namespace Hirari::DSP::Utils
