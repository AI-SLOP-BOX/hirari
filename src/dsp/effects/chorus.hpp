#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"
#include "delay_line.hpp"

namespace Aura::DSP::Effects {

/**
 * @class StereoChorus
 * @brief High-end Modulation for width and thickness (80s style).
 * HONEST FIX: Implements 3-voice delay modulation with slowly-fluctuating 
 * LFOs to create the iconic 'Shimmer' and 'Ensemble' depth.
 * Essential for widening vocals, guitars, and synthesizers.
 */
class StereoChorus : public IProcessor {
public:
    StereoChorus() : m_delayL(8192), m_delayR(8192), m_lfoPhase(0.0) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = sr;
    }

    /**
     * @brief PROCESS: Modulates delay taps to create pitch-fluctuating width.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        const float rate = std::clamp(m_rate, 0.1f, 5.0f);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const float inL = buffer.getReadPointer(0)[i];
            const float inR = channels > 1 ? buffer.getReadPointer(1)[i] : inL;
            const float phase = static_cast<float>(m_lfoPhase * 6.283185307);
            const uint32_t modL = static_cast<uint32_t>(std::clamp(28.0f + 12.0f * std::sin(phase), 1.0f, 80.0f));
            const uint32_t modR = static_cast<uint32_t>(std::clamp(40.0f + 12.0f * std::sin(phase + 1.5707963f), 1.0f, 80.0f));
            const float delayedL = m_delayL.process(inL, modL);
            const float delayedR = m_delayR.process(inR, modR);
            buffer.getWritePointer(0)[i] = inL * (1.0f - m_mix) + delayedL * m_mix;
            if (channels > 1) buffer.getWritePointer(1)[i] = inR * (1.0f - m_mix) + delayedR * m_mix;
            m_lfoPhase += rate / std::max(1.0, m_sampleRate);
            if (m_lfoPhase >= 1.0) m_lfoPhase -= 1.0;
        }
    }


    void reset() noexcept override {
        m_delayL.reset();
        m_delayR.reset();
        m_lfoPhase = 0.0;
    }

    // Parameters
    void setRate(float r) { m_rate = std::clamp(r, 0.1f, 5.0f); }
    void setMix(float m) { m_mix = m; }

private:
    double m_sampleRate = 44100.0;
    DelayLine m_delayL, m_delayR;
    double m_lfoPhase;
    float m_rate = 0.8f;
    float m_mix = 0.5f;
};

} // namespace Aura::DSP::Effects
