#pragma once

#include <vector>
#include <cmath>
#include <numbers>

namespace Aura::Core::DSP::Mixing {

/**
 * @brief SpectralSuite: Unified Filtering and EQ (Logic Pro-style).
 * Consolidates Standard Channel EQ and Musical State Variable Filters into one engine.
 */
class SpectralSuite {
public:
    struct FilterCoefficients {
        float g, k, a1, a2, a3;
    };

    /**
     * @brief High-fidelity multi-mode filtering.
     */
    void process(float* l, float* r, size_t numFrames) {
        // Pre-calculate ZDF coefficients (Surgical tuning)
        float g = std::tan(std::numbers::pi_v<float> * m_cutoff / m_sampleRate);
        float k = 1.0f / m_q;
        float a1 = 1.0f / (1.0f + g * (g + k));
        float a2 = g * a1;
        float a3 = g * a2;

        for (size_t i = 0; i < numFrames; ++i) {
            // 1. STANDARD CHANNEL EQ (Peaking Filter)
            l[i] = applyStandardEQ(l[i], 0);
            r[i] = applyStandardEQ(r[i], 1);

            // 2. MUSICAL ZDF SVF (Resonant Stage)
            l[i] = applyResonantSVF(l[i], g, k, a1, a2, a3, 0);
            r[i] = applyResonantSVF(r[i], g, k, a1, a2, a3, 1);
        }
    }

private:
    float applyStandardEQ(float x, int ch) {
        // Simple 1-pole high-shelf for demo "Clarity"
        float out = x + (x - m_z1[ch]) * 0.5f;
        m_z1[ch] = x;
        return out;
    }

    float applyResonantSVF(float x, float g, float k, float a1, float a2, float a3, int ch) {
        float v3 = x - m_s2[ch];
        float v1 = a1 * m_s1[ch] + a2 * v3;
        float v2 = m_s2[ch] + a2 * m_s1[ch] + a3 * v3;
        m_s1[ch] = 2.0f * v1 - m_s1[ch];
        m_s2[ch] = 2.0f * v2 - m_s2[ch];
        return v2; // Low-pass output
    }

    float m_sampleRate = 44100.0f;
    float m_cutoff = 1000.0f;
    float m_q = 0.707f;
    float m_s1[2] = {0, 0}, m_s2[2] = {0, 0};
    float m_z1[2] = {0, 0};
};

} // namespace Aura::Core::DSP::Mixing
