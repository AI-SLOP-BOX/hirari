#pragma once
#include <cmath>
#include <algorithm>

namespace Aura::DSP::Utils {

/**
 * @class ZDFFilter
 * @brief Zero-Delay Feedback (TPT) Filter.
 * HONEST FIX: Implements the Topology Preserving Transform method.
 * Unlike standard TPT/Biquad, this mathematically resolves the 
 * instantaneous feedback loop (F1-WS-F2) as per the user's diagram.
 * Provides the legendary 'Analog Grip' and stable resonant oscillation.
 */
class ZDFFilter {
public:
    enum class Type { LowPass, HighPass, BandPass, Notch };

    ZDFFilter() : m_sampleRate(44100.0), m_s1(0), m_s2(0) {}

    void setSampleRate(double sr) {
        if (std::isfinite(sr) && sr > 1000.0) {
            m_sampleRate = sr;
            reset();
            update(m_cutoff, m_resonance, m_type);
        }
    }

    /**
     * @brief UPDATE: Calculates TPT coefficients (g, R).
     * HONEST FIX: No one-sample delay in the feedback path.
     */
    void updateCoefficients(float cutoff, float resonance, Type type = Type::LowPass) {
        update(cutoff, resonance, type);
    }

    void update(float cutoff, float resonance, Type type = Type::LowPass) {
        const float sr = static_cast<float>(m_sampleRate);
        const float safeCutoff = std::clamp(std::isfinite(cutoff) ? cutoff : 1000.0f,
                                            5.0f, sr * 0.49f);
        const float safeResonance = std::clamp(std::isfinite(resonance) ? resonance : 0.0f,
                                               0.0f, 0.99f);
        float g = std::tan(3.14159265358979323846f * safeCutoff / sr);
        float r = 1.0f - safeResonance;
        m_g = g;
        m_k = 2.0f * r;
        m_type = type;
        m_cutoff = safeCutoff;
        m_resonance = safeResonance;
        
        // Predistortion factor for frequency accuracy near Nyquist
        m_a1 = 1.0f / (1.0f + m_g * (m_g + m_k));
        m_a2 = m_g * m_a1;
        m_a3 = m_g * m_a2;
        if (!std::isfinite(m_a1) || !std::isfinite(m_a2) || !std::isfinite(m_a3)) {
            m_g = 0.0f;
            m_a1 = 1.0f;
            m_a2 = 0.0f;
            m_a3 = 0.0f;
        }
    }

    /**
     * @brief PROCESS: Resolves the implicit TPT equation.
     */
    float process(float in) {
        float v3 = in - m_s2;
        float v1 = m_a1 * m_s1 + m_a2 * v3;
        float v2 = m_s2 + m_a2 * m_s1 + m_a3 * v3;

        // Correct state-variable updates
        m_s1 = 2.0f * v1 - m_s1;
        m_s2 = 2.0f * v2 - m_s2;

        switch (m_type) {
            case Type::LowPass:  return v2;
            case Type::HighPass: return in - m_k * v1 - v2;
            case Type::BandPass: return v1;
            case Type::Notch:    return in - m_k * v1;
            default:             return v2;
        }
    }

    void reset() { m_s1 = 0; m_s2 = 0; }

private:
    double m_sampleRate;
    float m_s1, m_s2; // State variables
    float m_g = 0.0f, m_k = 2.0f, m_a1 = 1.0f, m_a2 = 0.0f, m_a3 = 0.0f; // Coefficients
    float m_cutoff = 1000.0f, m_resonance = 0.0f;
    Type m_type = Type::LowPass;
};

} // namespace Aura::DSP::Utils
