#pragma once
#include <cmath>

namespace Aura::DSP::Analysis {

/**
 * @brief KWeightingFilter: Standardized frequency weighting for LUFS measurement.
 * Reference: ITU-R BS.1770-4.
 */
class KWeightingFilter {
public:
    KWeightingFilter(double sr = 44100.0) : m_sampleRate(sr) {
        setupCoefficients();
    }

    void setSampleRate(double sr) {
        m_sampleRate = sr;
        setupCoefficients();
        reset();
    }

    void reset() noexcept {
        m_left = {};
        m_right = {};
    }

    void process(float l, float r, float& outL, float& outR) {
        outL = processChannel(l, m_left);
        outR = processChannel(r, m_right);
    }

private:
    void setupCoefficients() {
        const double safeRate = (std::isfinite(m_sampleRate) && m_sampleRate > 1000.0)
            ? m_sampleRate : 44100.0;
        const double x = std::exp(-2.0 * 3.141592653589793 * 38.1358 / safeRate);
        const double y = std::exp(-2.0 * 3.141592653589793 * 1681.974 / safeRate);
        m_hpA = static_cast<float>(x);
        m_shelfA = static_cast<float>(y);
    }

    struct State { float hp = 0.0f; float shelf = 0.0f; float previous = 0.0f; };

    float processChannel(float input, State& state) noexcept {
        if (!std::isfinite(input)) input = 0.0f;
        const float hp = input - state.previous + m_hpA * state.hp;
        state.previous = input;
        state.hp = hp;
        const float shelf = hp + (1.0f - m_shelfA) * (hp - state.shelf) * 0.5f;
        state.shelf = shelf;
        return std::isfinite(shelf) ? shelf : 0.0f;
    }


    double m_sampleRate;
    float m_hpA = 0.95f;
    float m_shelfA = 0.8f;
    State m_left{};
    State m_right{};
};

} // namespace Aura::DSP::Analysis
