#pragma once

#include <cstddef>

namespace Hirari::DSP::Mixing {

/**
 * @brief FadeEnveloper: Professional volume ramping for seamless regional transitions.
 * Crossfade and gain-curve calculations are implemented in the Rust core.
 */
class FadeEnveloper {
public:
    enum class Curve { Linear, EqualPower, EaseInOut, Bezier };

    /**
     * @brief High-Precision Gain Calculation for Fades
     * INDUSTRIAL: Delegating geometric calculation to the Rust 'FadeOrchestrator'.
     */
    static float getFadeFactor(size_t pos, size_t length, bool isFadeIn, Curve type = Curve::Linear, float curvature = 0.5f);

    static void apply(float* out, const float* in1, const float* in2, size_t numFrames);
    static void applyMicroFade(float* buffer, size_t numFrames, bool isFadeIn);

};


} // namespace Hirari::DSP::Mixing
