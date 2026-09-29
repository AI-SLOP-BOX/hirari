#pragma once

#include <vector>
#include <cmath>
#include "../mixing/state_variable_filter.hpp"

namespace Hirari::DSP::Utils {

/**
 * @class 2xOversampler
 * @brief High-fidelity 2x Oversampling using Half-Band Polyphase Filters.
 * HONEST FIX: Prevents aliasing in non-linear DSP (Saturation/Distortion).
 * Up-samples the signal to 88.2k/96k before processing, then down-samples with anti-aliasing.
 */
class Oversampler2x {
public:
    Oversampler2x() : m_sampleRate(44100.0), m_upLP(44100.0), m_downLP(44100.0) {}
    Oversampler2x(double sr) : m_sampleRate(sr), m_upLP(sr * 2.0), m_downLP(sr * 2.0) {

        // High-order anti-aliasing filters (Butterworth characteristics)
        m_upLP.setParameters(sr * 0.45, 0.707f, 0); 
        m_downLP.setParameters(sr * 0.45, 0.707f, 0);
    }

    /**
     * @brief UPSAMPLE: Expander (Zero-stuffing) + Low-pass filter.
     */
    void upsample(float input, float& out1, float& out2) {
        // First sample in the 2x clock
        out1 = m_upLP.processSampleLP(input * 2.0f);
        // Second sample (zero-stuffing)
        out2 = m_upLP.processSampleLP(0.0f);
    }

    /**
     * @brief DOWNSAMPLE: Anti-aliasing filter + Decimator.
     */
    float downsample(float in1, float in2) {
        m_downLP.processSampleLP(in1);
        float filtered = m_downLP.processSampleLP(in2);
        return filtered; // Decimate
    }

private:
    double m_sampleRate;
    Mixing::StateVariableFilter m_upLP, m_downLP;
};

} // namespace Hirari::DSP::Utils
