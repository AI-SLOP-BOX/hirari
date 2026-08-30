#pragma once

#include <vector>
#include <cmath>
#include <numbers>

namespace Aura::Core::DSP::Mixing {

/**
 * @brief High-Fidelity Biquad Filter: Professional RBJ implementation.
 */
class BiquadFilter {
public:
    void process(float* left, float* right, size_t numFrames) noexcept {
        if (!left || !right) return;
        for (size_t i = 0; i < numFrames; ++i) {
            left[i] = processSample(left[i], m_z1L, m_z2L);
            right[i] = processSample(right[i], m_z1R, m_z2R);
        }
    }

    /**
     * @brief CALC: Resolves biquad coefficients with industrial precision.
     */
    void calculatePeaking(float freq, float sr, float Q, float gainDB) {
        if (!std::isfinite(freq) || !std::isfinite(sr) || !std::isfinite(Q) ||
            !std::isfinite(gainDB) || sr <= 1000.0f || Q <= 0.0f) return;
        const float f = std::clamp(freq, 10.0f, sr * 0.49f);
        constexpr float kPi = 3.14159265358979323846f;
        const float omega = 2.0f * kPi * f / sr;
        const float alpha = std::sin(omega) / (2.0f * Q);
        const float A = std::pow(10.0f, std::clamp(gainDB, -24.0f, 24.0f) / 40.0f);
        const float b0 = 1.0f + alpha * A;
        const float b1 = -2.0f * std::cos(omega);
        const float b2 = 1.0f - alpha * A;
        const float a0 = 1.0f + alpha / A;
        const float a1 = b1;
        const float a2 = 1.0f - alpha / A;
        m_b0 = b0 / a0; m_b1 = b1 / a0; m_b2 = b2 / a0;
        m_a1 = a1 / a0; m_a2 = a2 / a0;
    }

    void reset() noexcept { m_z1L = m_z2L = m_z1R = m_z2R = 0.0f; }

private:
    static float sanitize(float value) noexcept { return std::isfinite(value) ? value : 0.0f; }
    float processSample(float input, float& z1, float& z2) const noexcept {
        const float x = sanitize(input);
        const float y = m_b0 * x + z1;
        z1 = m_b1 * x - m_a1 * y + z2;
        z2 = m_b2 * x - m_a2 * y;
        return sanitize(y);
    }

    float m_b0 = 1.0f, m_b1 = 0.0f, m_b2 = 0.0f;
    float m_a1 = 0.0f, m_a2 = 0.0f;
    float m_z1L = 0.0f, m_z2L = 0.0f, m_z1R = 0.0f, m_z2R = 0.0f;
};

} // namespace Aura::Core::DSP::Mixing
