#pragma once

#include <cmath>
#include <cstdint>

namespace Aura::DSP::Math {

/**
 * @brief FastMath: High-performance mathematical approximations for Pro DSP.
 * Addresses the heavy CPU load of standard std::exp and std::pow.
 */
class FastMath {
public:
    /**
     * @brief Fast approximation of exponential function.
     * Based on Schraudolph's algorithm (1998).
     */
    static inline float fastExp(float x) {
        union { float f; int32_t i; } u;
        u.i = static_cast<int32_t>(12102203.0f * x + 1064866805.0f);
        return u.f;
    }

    /**
     * @brief Fast dB to Linear conversion.
     */
    static inline float dbToLinear(float db) {
        return fastExp(db * 0.1151292546497022842f); // db * ln(10)/20
    }

    /**
     * @brief Fast Linear to dB conversion.
     */
    static inline float linearToDb(float linear) {
        return 20.0f * std::log10(linear + 1e-10f); // FastLog could be added
    }
};

} // namespace Aura::DSP::Math
