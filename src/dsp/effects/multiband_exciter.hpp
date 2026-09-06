#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include <array>
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
    MultibandExciter(double sr = 44100.0) : m_sampleRate(44100.0), m_satLow(44100.0), m_satMid(44100.0), m_satHigh(44100.0) {
        setSampleRate(sr);
    }

    void setupCrossover(float lowCut, float highCut) {
        for (auto& filter : m_lowPass) filter.setParameters(lowCut, 0.707f, 0); // Low band LP
        for (auto& filter : m_midLowPass) filter.setParameters(highCut, 0.707f, 0); // Mid band LP
    }

    void process(float* l, float* r, uint32_t numSamples) {
        if (!l || !r) return;
        for (uint32_t i = 0; i < numSamples; ++i) {
            const float inL = std::isfinite(l[i]) ? l[i] : 0.0f;
            const float inR = std::isfinite(r[i]) ? r[i] : 0.0f;
            const float lowL = m_lowPass[0].processSampleLP(inL);
            const float lowR = m_lowPass[1].processSampleLP(inR);
            const float low = 0.5f * (lowL + lowR);
            const float midL = m_midLowPass[0].processSampleLP(inL) - lowL;
            const float midR = m_midLowPass[1].processSampleLP(inR) - lowR;
            const float highL = inL - lowL - midL;
            const float highR = inR - lowR - midR;
            const float exciteL = lowL + 0.35f * std::tanh(midL * 1.4f) + 0.5f * std::tanh(highL * 2.0f);
            const float exciteR = lowR + 0.35f * std::tanh(midR * 1.4f) + 0.5f * std::tanh(highR * 2.0f);
            const float outL = 0.75f * inL + 0.25f * exciteL;
            const float outR = 0.75f * inR + 0.25f * exciteR;
            if (r == l) {
                const float mono = 0.5f * (outL + outR);
                l[i] = std::isfinite(mono) ? mono : 0.0f;
            } else {
                l[i] = std::isfinite(outL) ? outL : 0.0f;
                r[i] = std::isfinite(outR) ? outR : 0.0f;
            }
        }
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override { (void)bs; setSampleRate(sr); reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi; (void)context;
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        float* l = buffer.getWritePointer(0);
        float* r = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : l;
        process(l, r, buffer.getNumSamples());
    }
    void reset() noexcept override { for (auto& filter : m_lowPass) filter.reset(); for (auto& filter : m_midLowPass) filter.reset(); }


    void setSampleRate(double sr) {
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44'100.0;
        for (auto& filter : m_lowPass) filter.setSampleRate(m_sampleRate);
        for (auto& filter : m_midLowPass) filter.setSampleRate(m_sampleRate);
        m_satLow.prepareToPlay(m_sampleRate, 0);
        m_satMid.prepareToPlay(m_sampleRate, 0);
        m_satHigh.prepareToPlay(m_sampleRate, 0);
        setupCrossover(200.0f, 3000.0f);
    }
    uint32_t getLatency() const { return 0; }

private:
    double m_sampleRate;
    std::array<Mixing::StateVariableFilter, 2> m_lowPass, m_midLowPass;
    AnalogSaturator m_satLow, m_satMid, m_satHigh;
};

} // namespace Aura::DSP::Effects
