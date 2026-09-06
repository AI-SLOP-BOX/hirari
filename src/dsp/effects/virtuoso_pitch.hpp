#pragma once
#include <cmath>
#include <algorithm>
#include <array>
#include "../../core/audio_buffer.hpp"
#include "../../core/midi_buffer.hpp"
#include "../iprocessor.hpp"
#include "../utils/pitch_shifter.hpp"

namespace Aura::DSP::Effects {

/**
 * @class VirtuosoPitch
 * @brief Real-time Intelligent Pitch Correction.
 */
class VirtuosoPitch : public IProcessor {
public:
    VirtuosoPitch(double sr = 44100.0) : m_sampleRate(sr) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        if (std::isfinite(sr) && sr > 1000.0) m_sampleRate = sr;
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi; (void)context;
        if (buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        float* l = buffer.getWritePointer(0);
        float* r = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : l;
        if (!l || !r) return;
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const float x = std::isfinite(l[i]) ? l[i] : 0.0f;
            if (m_lastIn <= 0.0f && x > 0.0f && i > m_lastCross) {
                const uint64_t period = i - m_lastCross;
                if (period > 4 && period < static_cast<uint64_t>(m_sampleRate / 20.0)) {
                    m_detectedPitch = static_cast<float>(m_sampleRate / static_cast<double>(period));
                    m_targetPitch = snapToScale(m_detectedPitch);
                    m_lastCross = i;
                }
            }
            m_lastIn = x;
        }
        const float ratio = std::clamp(std::isfinite(m_detectedPitch) && m_detectedPitch > 20.0f
            ? m_targetPitch / m_detectedPitch : 1.0f, 0.5f, 2.0f);
        m_shifterL.process(l, buffer.getNumSamples(), ratio, static_cast<float>(m_sampleRate));
        if (r != l) m_shifterR.process(r, buffer.getNumSamples(), ratio, static_cast<float>(m_sampleRate));
    }


    void reset() noexcept override {
        m_writeCount = 0;
        m_lastCross = 0;
        m_shifterL.reset();
        m_shifterR.reset();
        m_lastIn = 0.0f;
        m_detectedPitch = m_targetPitch = 440.0f;
    }

private:
    float snapToScale(float freq) {
        // Find nearest semitone (A4 = 440)
        float semitones = 69.0f + 12.0f * std::log2(freq / 440.0f);
        float nearest = std::round(semitones);
        return 440.0f * std::pow(2.0f, (nearest - 69.0f) / 12.0f);
    }

    double m_sampleRate;
    Utils::PitchShifter m_shifterL, m_shifterR;
    uint64_t m_writeCount = 0, m_lastCross = 0;
    float m_lastIn = 0.0f;
    float m_detectedPitch = 440.0f, m_targetPitch = 440.0f;
};

} // namespace Aura::DSP::Effects
