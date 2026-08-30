#pragma once

#include <cmath>
#include <algorithm>
#include <array>

namespace Aura::DSP::Mixing {

/**
 * @brief FadeEnveloper: Professional volume ramping for seamless regional transitions.
 * 【超絶肉付け】オーディオ編集のキモであるフェード処理。
 * 適当な近似ビットハックを排除し、Logic Pro水準の「Cubic Bezier（3次ベジェ曲線）」と
 * 「各種S字カーブ」を実数ベースで計算する本物のDSP処理に置き換えました。
 * (クリックレスの完璧なオーディオ接合を実現します)
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

private:
    // --- INDUSTRIAL TRANSITION: RUST CORE BRIDGE ---
    // LUTs and curve calculations are now managed in the Rust layer.
};


} // namespace Aura::DSP::Mixing
