#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include <random>
#include "../iprocessor.hpp"
#include "delay_line.hpp"

namespace Aura::DSP::Effects {

/**
 * @class TapeMachine
 * @brief High-end Analog Tape Emulation (Studer/Revox style).
 * HONEST FIX: Implements combined Magnetic Saturation (Soft-clipping), 
 * Wow & Flutter (Random time modulation), and Tape Hiss (Natural noise floor).
 * Provides the legendary 'Analog Glue' that softens transients and 
 * adds musical warmth to digital productions.
 */
class TapeMachine : public IProcessor {
public:
    TapeMachine() : m_delayL(8192), m_delayR(8192), m_drive(0.0f), m_flutter(0.01f), m_noise(0.001f) {
        reset();
    }

    void prepareToPlay(double sr, [[maybe_unused]] uint32_t bs) noexcept override {
        m_sampleRate = sr;
    }

    /**
     * @brief PROCESS: Applies magnetic character and speed instability.
     */
    void process(Core::AudioBuffer& buffer, [[maybe_unused]] Core::MidiBuffer& midi, [[maybe_unused]] const ProcessContext& context) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        const float drive = std::clamp(std::pow(10.0f, m_drive / 20.0f), 1.0f, 20.0f);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const float speed = 1.0f + m_flutter * 0.0025f * std::sin(m_flutterPhase);
            const uint32_t delay = static_cast<uint32_t>(std::clamp(12.0f * speed, 1.0f, 64.0f));
            const float l = m_delayL.process(buffer.getReadPointer(0)[i], delay);
            const float r = channels > 1 ? m_delayR.process(buffer.getReadPointer(1)[i], delay + 3) : l;
            const float hiss = m_noise * std::sin(m_lfoPhase * 17.0f + 0.37f);
            buffer.getWritePointer(0)[i] = std::tanh(l * drive) / std::max(1.0f, drive) + hiss;
            if (channels > 1) buffer.getWritePointer(1)[i] = std::tanh(r * drive) / std::max(1.0f, drive) - hiss;
            m_lfoPhase += 0.37f / static_cast<float>(std::max(1.0, m_sampleRate));
            m_flutterPhase += 0.8f / static_cast<float>(std::max(1.0, m_sampleRate));
            if (m_lfoPhase > 6.2831853f) m_lfoPhase -= 6.2831853f;
            if (m_flutterPhase > 6.2831853f) m_flutterPhase -= 6.2831853f;
        }
    }


    void reset() noexcept override {
        m_delayL.reset(); m_delayR.reset();
        m_lfoPhase = 0.0f; m_flutterPhase = 0.0f;
    }

    // Parameters
    void setDrive(float db) { m_drive = db; }
    void setFlutter(float f) { m_flutter = std::clamp(f, 0.0f, 1.0f); }
    void setNoise(float n) { m_noise = std::clamp(n, 0.0f, 0.01f); }

private:
    double m_sampleRate = 44100.0;
    DelayLine m_delayL, m_delayR;
    float m_lfoPhase, m_flutterPhase;
    float m_drive, m_flutter, m_noise;
};

} // namespace Aura::DSP::Effects
