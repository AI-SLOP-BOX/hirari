#pragma once
#include <cmath>

namespace Hirari::Core::DSP::Mixing {

/**
 * @brief LinkwitzRileyFilter: 4th-order (24dB/oct) phase-aligned crossover.
 * Essential for high-end multiband processing.
 */
class LinkwitzRileyFilter {
public:
    void setParameters(float cutoff, double sr) {
        float wc = 2.0f * M_PI * cutoff;
        float wc2 = wc * wc;
        float wc3 = wc2 * wc;
        float wc4 = wc2 * wc2;
        float k = wc / std::tan(M_PI * cutoff / sr);
        float k2 = k * k;
        float k3 = k2 * k;
        float k4 = k2 * k2;
        float sqrt2 = std::sqrt(2.0f);

        float den = k4 + 2.0f * sqrt2 * k3 * wc + 4.0f * k2 * wc2 + 2.0f * sqrt2 * k * wc3 + wc4;
        
        m_b0 = wc4 / den;
        m_b1 = 4.0f * wc4 / den;
        m_b2 = 6.0f * wc4 / den;
        m_b3 = 4.0f * wc4 / den;
        m_b4 = wc4 / den;

        m_a1 = (4.0f * k4 + 4.0f * sqrt2 * k3 * wc - 4.0f * sqrt2 * k * wc3 - 4.0f * wc4) / den;
        m_a2 = (6.0f * k4 - 8.0f * k2 * wc2 + 6.0f * wc4) / den;
        m_a3 = (4.0f * k4 - 4.0f * sqrt2 * k3 * wc + 4.0f * sqrt2 * k * wc3 - 4.0f * wc4) / den;
        m_a4 = (k4 - 2.0f * sqrt2 * k3 * wc + 4.0f * k2 * wc2 - 2.0f * sqrt2 * k * wc3 + wc4) / den;
    }

    float process(float x) {
        float out = m_b0 * x + m_b1 * m_z1 + m_b2 * m_z2 + m_b3 * m_z3 + m_b4 * m_z4 - m_a1 * m_y1 - m_a2 * m_y2 - m_a3 * m_y3 - m_a4 * m_y4;
        m_z4 = m_z3; m_z3 = m_z2; m_z2 = m_z1; m_z1 = x;
        m_y4 = m_y3; m_y3 = m_y2; m_y2 = m_y1; m_y1 = out;
        return out;
    }

    // Highpass is just Input delayed - Lowpass for LR filters
    // Or we can use separate coefficients for truly symmetric LR4
private:
    float m_b0=0, m_b1=0, m_b2=0, m_b3=0, m_b4=0;
    float m_a1=0, m_a2=0, m_a3=0, m_a4=0;
    float m_z1=0, m_z2=0, m_z3=0, m_z4=0;
    float m_y1=0, m_y2=0, m_y3=0, m_y4=0;
};

}
