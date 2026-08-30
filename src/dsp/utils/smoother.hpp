#pragma once
#include <cmath>
#include <atomic>

namespace Aura::DSP::Utils {

/**
 * @class LinearSmoother
 * @brief Industrial Parameter Smoother.
 * Eliminates audio clicks by interpolating parameter changes over a fixed time.
 */
class LinearSmoother {
public:
    LinearSmoother() = default;

    void reset(double sampleRate, double timeMs) {
        m_sampleRate = sampleRate;
        m_stepSamples = static_cast<uint32_t>(sampleRate * (timeMs / 1000.0));
        m_count = 0;
    }

    void setTarget(float target) {
        if (std::abs(target - m_target) < 1e-6f) return;
        m_target = target;
        m_start = m_current;
        m_inc = (m_target - m_start) / static_cast<float>(std::max(1u, m_stepSamples));
        m_count = m_stepSamples;
    }

    float getNextValue() {
        if (m_count > 0) {
            m_current += m_inc;
            m_count--;
        } else {
            m_current = m_target;
        }
        return m_current;
    }

    void skip(uint32_t n) {
        for(uint32_t i=0; i<n; ++i) getNextValue();
    }

    float getCurrentValue() const { return m_current; }

private:
    float m_current = 0.0f;
    float m_target = 0.0f;
    float m_start = 0.0f;
    float m_inc = 0.0f;
    uint32_t m_stepSamples = 441; // Default 10ms @ 44.1k
    uint32_t m_count = 0;
    double m_sampleRate = 44100.0;
};

} // namespace Aura::DSP::Utils
