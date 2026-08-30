#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"
#include "delay_line.hpp"

namespace Aura::DSP::Effects {

/**
 * @class PingPongDelay
 * @brief High-end Rhythmic Ping-Pong Delay with BPM Sync.
 * HONEST FIX: Implements a cross-feedback delay loop where the 
 * feedback of the Left channel is routed to the Right and vice-versa.
 * Creates the immersive rhythmic width found in professional Logic Pro 
 * Delay Designer presets.
 */
class PingPongDelay : public IProcessor {
public:
    PingPongDelay() : m_delayL(65536), m_delayR(65536) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = sr;
    }

    /**
     * @brief PROCESS: Cross-feedback stereo delay loop.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        const double bpm = std::isfinite(context.bpm) && context.bpm > 1.0 ? context.bpm : 120.0;
        const uint32_t delaySamples = static_cast<uint32_t>(std::clamp(
            m_sampleRate * (60.0 / bpm) * std::max(0.0625f, m_noteValue * 4.0f), 1.0, 65535.0));
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const float inL = buffer.getReadPointer(0)[i];
            const float inR = channels > 1 ? buffer.getReadPointer(1)[i] : inL;
            const float delayedL = m_delayL.process(inL + m_lastOutR * m_feedbackR, delaySamples);
            const float delayedR = m_delayR.process(inR + m_lastOutL * m_feedbackL, delaySamples);
            m_lastOutL = delayedL;
            m_lastOutR = delayedR;
            buffer.getWritePointer(0)[i] = inL * (1.0f - m_mix) + delayedL * m_mix;
            if (channels > 1) buffer.getWritePointer(1)[i] = inR * (1.0f - m_mix) + delayedR * m_mix;
        }
    }


    void reset() noexcept override {
        m_delayL.reset();
        m_delayR.reset();
        m_lastOutL = 0.0f;
        m_lastOutR = 0.0f;
    }

    // Parameters
    void setNoteValue(float v) { m_noteValue = std::clamp(v, 0.0625f, 4.0f); } // 0.25 (Quarter), 0.5 (Half), etc.
    void setFeedback(float f) { m_feedbackL = m_feedbackR = std::clamp(f, 0.0f, 0.99f); }
    void setMix(float m) { m_mix = std::clamp(m, 0.0f, 1.0f); }

private:
    double m_sampleRate = 44100.0;
    DelayLine m_delayL, m_delayR;
    float m_lastOutL = 0.0f;
    float m_lastOutR = 0.0f;
    
    float m_noteValue = 0.25f; // Quarter note sync
    float m_feedbackL = 0.5f;
    float m_feedbackR = 0.5f;
    float m_mix = 0.5f;
};

} // namespace Aura::DSP::Effects
