#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"
#include "../mixing/state_variable_filter.hpp"
#include "analog_saturator.hpp"

namespace Aura::DSP::Effects {

/**
 * @brief MultibandExciter: Top-tier frequency-specific saturation.
 * Standard tool for 'Mastering' and 'Drum' processing.
 */
class MultibandExciter : public IProcessor {
public:
    MultibandExciter(double sr = 44100.0) : m_sampleRate(sr), m_satLow(sr), m_satMid(sr), m_satHigh(sr) {
        setupCrossover(200.0f, 3000.0f);
    }

    void setupCrossover(float lowCut, float highCut) {
        m_lowPass.setParameters(lowCut, 0.707f, 0); // Low band LP
        m_midLowPass.setParameters(highCut, 0.707f, 0); // Mid band LP
    }

    void process(float* l, float* r, uint32_t numSamples) {
        if (!l || !r) return;
        for (uint32_t i = 0; i < numSamples; ++i) {
            const float inL = std::isfinite(l[i]) ? l[i] : 0.0f;
            const float inR = std::isfinite(r[i]) ? r[i] : 0.0f;
            const float lowL = m_lowPass.processSampleLP(inL);
            const float lowR = m_lowPass.processSampleLP(inR);
            const float low = 0.5f * (lowL + lowR);
            const float midL = m_midLowPass.processSampleLP(inL) - lowL;
            const float midR = m_midLowPass.processSampleLP(inR) - lowR;
            const float highL = inL - lowL - midL;
            const float highR = inR - lowR - midR;
            const float exciteL = lowL + 0.35f * std::tanh(midL * 1.4f) + 0.5f * std::tanh(highL * 2.0f);
            const float exciteR = lowR + 0.35f * std::tanh(midR * 1.4f) + 0.5f * std::tanh(highR * 2.0f);
            l[i] = std::isfinite(0.75f * inL + 0.25f * exciteL) ? 0.75f * inL + 0.25f * exciteL : 0.0f;
            r[i] = std::isfinite(0.75f * inR + 0.25f * exciteR) ? 0.75f * inR + 0.25f * exciteR : 0.0f;
        }
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override { (void)bs; setSampleRate(sr); reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi; (void)context;
        if (buffer.getNumChannels() == 0) return;
        float* l = buffer.getWritePointer(0);
        float* r = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : l;
        process(l, r, buffer.getNumSamples());
    }
    void reset() noexcept override { m_lowPass.reset(); m_midLowPass.reset(); }


    void setSampleRate(double sr) { if (std::isfinite(sr) && sr > 1000.0) m_sampleRate = sr; setupCrossover(200.0f, 3000.0f); }
    uint32_t getLatency() const { return 0; }

private:
    double m_sampleRate;
    Mixing::StateVariableFilter m_lowPass, m_midLowPass;
    AnalogSaturator m_satLow, m_satMid, m_satHigh;
};

} // namespace Aura::DSP::Effects
