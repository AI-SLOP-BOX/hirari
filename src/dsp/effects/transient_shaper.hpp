#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class TransientShaper
 * @brief Dynamic Envelope modification for Percussion and Drums.
 * HONEST FIX: Implements dual-envelope detection (Fast Attack vs slow envelope)
 * to isolate and boost/cut the initial crack of a drum sound.
 * Provides the 'Snap' and 'Weight' found in professional SSL-style transient designers.
 */
class TransientShaper : public IProcessor {
public:
    TransientShaper() : m_attackEnv(0.0f), m_sustainEnv(0.0f) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = sr;
        updateBallistics();
    }

    /**
     * @brief PROCESS: Dynamically reshapes the signal's attack and tail.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        const float attackGain = std::clamp(1.0f + m_attack, 0.0f, 2.0f);
        const float sustainGain = std::clamp(1.0f + m_sustain, 0.0f, 2.0f);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const float l = buffer.getReadPointer(0)[i];
            const float r = channels > 1 ? buffer.getReadPointer(1)[i] : l;
            const float level = std::max(std::abs(l), std::abs(r));
            m_attackEnv = m_attackAlpha * m_attackEnv + (1.0f - m_attackAlpha) * level;
            m_sustainEnv = m_sustainAlpha * m_sustainEnv + (1.0f - m_sustainAlpha) * level;
            const float transient = std::clamp(m_attackEnv - m_sustainEnv, -1.0f, 1.0f);
            const float body = std::clamp(m_sustainEnv, 0.0f, 1.0f);
            const float gain = std::clamp(1.0f + transient * (attackGain - 1.0f) + body * (sustainGain - 1.0f), 0.0f, 3.0f);
            buffer.getWritePointer(0)[i] = l * gain;
            if (channels > 1) buffer.getWritePointer(1)[i] = r * gain;
        }
    }


    void reset() noexcept override {
        m_attackEnv = 0.0f;
        m_sustainEnv = 0.0f;
        m_currentGain = 1.0f;
    }

    // Parameters (-1.0 to 1.0)
    void setAttack(float a) { m_attack = a; }
    void setSustain(float s) { m_sustain = s; }

private:
    void updateBallistics() {
        // Attack envelope: Fast (approx 5ms)
        m_attackAlpha = std::exp(-1.0f / (m_sampleRate * 0.005f));
        // Sustain envelope: Slow (approx 50ms)
        m_sustainAlpha = std::exp(-1.0f / (m_sampleRate * 0.050f));
    }

    double m_sampleRate = 44100.0;
    float m_attack = 0.0f;
    float m_sustain = 0.0f;

    float m_attackEnv = 0.0f;
    float m_sustainEnv = 0.0f;
    float m_attackAlpha = 0.9f;
    float m_sustainAlpha = 0.99f;
    float m_currentGain = 1.0f;
};

} // namespace Aura::DSP::Effects
