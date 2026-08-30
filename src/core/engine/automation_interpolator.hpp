#pragma once
#include <cmath>
#include <algorithm>

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

namespace Aura::Core::Engine {

/**
 * @struct AutomationInterpolator
 * @brief Interpolation Geometry Hub.
 * Implements professional Bezier, Exponential, and Sine curves for automation.
 */
struct AutomationInterpolator {
    static constexpr float lerp(float a, float b, float t) noexcept {
        return a + t * (b - a);
    }

    /**
     * @brief CUBIC BEZIER: Ease-in / Ease-out transition curves parameterized by curvature.
     */
    static float cubicBezier(float a, float b, float t, float curvature) noexcept {
        t = std::clamp(t, 0.0f, 1.0f);
        float t_curved = t;
        if (curvature > 0.0f) {
            // Ease-in: slow start, fast end
            t_curved = std::pow(t, 1.0f + curvature * 3.0f);
        } else if (curvature < 0.0f) {
            // Ease-out: fast start, slow end
            t_curved = 1.0f - std::pow(1.0f - t, 1.0f - curvature * -3.0f);
        }
        return lerp(a, b, t_curved);
    }

    /**
     * @brief EXPONENTIAL: Exponential curve based on curvature factor.
     */
    static float exponential(float a, float b, float t, float curvature) noexcept {
        t = std::clamp(t, 0.0f, 1.0f);
        if (std::abs(curvature) < 1e-4f) {
            return lerp(a, b, t);
        }
        float alpha = curvature * 5.0f;
        float denom = std::exp(alpha) - 1.0f;
        if (std::abs(denom) < 1e-4f) return lerp(a, b, t);
        float t_curved = (std::exp(alpha * t) - 1.0f) / denom;
        return lerp(a, b, t_curved);
    }

    /**
     * @brief SINE CURVE: Smooth sinusoidal S-curve interpolation.
     */
    static float sineCurve(float t) noexcept {
        t = std::clamp(t, 0.0f, 1.0f);
        return 0.5f * (1.0f - std::cos(static_cast<float>(M_PI) * t));
    }
};

} // namespace Aura::Core::Engine
