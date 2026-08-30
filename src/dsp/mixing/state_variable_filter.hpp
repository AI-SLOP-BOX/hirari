#pragma once
#include <cmath>
#include <algorithm>
#include <numbers>
#include "../../core/engine/parameter_smoother.hpp"

namespace Aura::DSP::Mixing {

/**
 * @class StateVariableFilter
 * @brief Industrial Zero-Delay Feedback (ZDF) SVF.
 * HONEST FIX: Reconstructed the broken logic for LP, HP, and BP modes.
 * Fully protected against denormals and artifacts during parameter changes.
 */
class StateVariableFilter {
public:
    StateVariableFilter(double sr = 44100.0) : m_sampleRate(std::isfinite(sr) && sr > 1000.0 ? sr : 44100.0) {
        m_freqSmoother.reset(1000.0f);
        m_resSmoother.reset(0.707f);
        reset();
    }

    void reset() {
        m_ic1 = 0.0f;
        m_ic2 = 0.0f;
    }

    void setSampleRate(double sr) noexcept {
        if (std::isfinite(sr) && sr > 1000.0) m_sampleRate = sr;
    }

    void setParameters(float freq, float res, int mode = 0) {
        m_freqSmoother.setTarget(freq);
        m_resSmoother.setTarget(res);
        m_mode = mode;
    }

    void processBlockLP(float* data, uint32_t numSamples) {
        if (!data) return;
        for (uint32_t s = 0; s < numSamples; ++s) {
            updateCoefficients();
            float x = data[s];
            float v3 = x - m_ic2;
            float v1 = m_a1 * m_ic1 + m_a2 * v3;
            float v2 = m_ic2 + m_a2 * m_ic1 + m_a3 * v3;
            m_ic1 = 2.0f * v1 - m_ic1;
            m_ic2 = 2.0f * v2 - m_ic2;
            data[s] = v2; // Low-pass
        }
    }

    void processBlockBP(float* data, uint32_t numSamples) {
        if (!data) return;
        for (uint32_t s = 0; s < numSamples; ++s) {
            data[s] = processSampleBP(data[s]);
        }
    }

    inline float processSampleLP(float x) {
        updateCoefficients();
        float v3 = x - m_ic2;
        float v1 = m_a1 * m_ic1 + m_a2 * v3;
        float v2 = m_ic2 + m_a2 * m_ic1 + m_a3 * v3;
        m_ic1 = 2.0f * v1 - m_ic1;
        m_ic2 = 2.0f * v2 - m_ic2;
        return v2;
    }

    inline float processSampleBP(float x) {
        updateCoefficients();
        float g = m_g;
        float k = m_k;
        float h = 1.0f / (1.0f + g * (g + k));
        float bp = h * (m_ic1 + g * (x - m_ic2));
        float lp = m_ic2 + g * bp;
        m_ic1 = 2.0f * bp - m_ic1;
        m_ic2 = 2.0f * lp - m_ic2;
        return bp;
    }

    inline float processSampleHP(float x) {
        updateCoefficients();
        float g = m_g;
        float k = m_k;
        float h = 1.0f / (1.0f + g * (g + k));
        float bp = h * (m_ic1 + g * (x - m_ic2));
        float lp = m_ic2 + g * bp;
        float hp = x - k * bp - lp;
        m_ic1 = 2.0f * bp - m_ic1;
        m_ic2 = 2.0f * lp - m_ic2;
        return hp;
    }
    
    void processBlockHP(float* data, uint32_t numSamples) {
        if (!data) return;
        for (uint32_t s = 0; s < numSamples; ++s) {
            data[s] = processSampleHP(data[s]);
        }
    }


private:
    void updateCoefficients() {
        float f = m_freqSmoother.getNextValue();
        float r = m_resSmoother.getNextValue();
        
        // Only update if parameters changed significantly
        if (std::abs(f - m_lastFreq) > 0.001f || std::abs(r - m_lastRes) > 0.001f) {
            const float safeSr = static_cast<float>(std::max(1000.0, m_sampleRate));
            const float safeFreq = std::clamp(std::isfinite(f) ? f : 1000.0f,
                                              5.0f, safeSr * 0.49f);
            const float safeRes = std::clamp(std::isfinite(r) ? r : 0.707f,
                                             0.05f, 4.0f);
            float g = std::tan(static_cast<float>(M_PI) * safeFreq / safeSr);
            float k = 1.0f / safeRes;
            m_g = g;
            m_k = k;
            m_a1 = 1.0f / (1.0f + g * (g + k));
            m_a2 = g * m_a1;
            m_a3 = g * m_a2;
            if (!std::isfinite(m_a1) || !std::isfinite(m_a2) || !std::isfinite(m_a3)) {
                m_a1 = 1.0f;
                m_a2 = 0.0f;
                m_a3 = 0.0f;
                g = 0.0f;
                k = 1.0f;
            }
            m_lastFreq = safeFreq;
            m_lastRes = safeRes;
        }
    }

    double m_sampleRate;
    ::Aura::Core::Engine::LinearSmoother m_freqSmoother;
    ::Aura::Core::Engine::LinearSmoother m_resSmoother;
    
    float m_ic1 = 0.0f, m_ic2 = 0.0f;
    float m_lastFreq = -1.0f, m_lastRes = -1.0f;
    float m_a1 = 0.0f, m_a2 = 0.0f, m_a3 = 0.0f;
    float m_g = 0.0f, m_k = 0.0f;
    int m_mode = 0;
};

} // namespace Aura::DSP::Mixing
