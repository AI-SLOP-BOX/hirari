#pragma once
#include <cmath>
#include <algorithm>
#include <vector>
#include "../../core/audio_buffer.hpp"
#include "../../core/midi_buffer.hpp"
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class SidechainCompressor
 * @brief Professional High-performance Ducking Engine using the Sidechain input.
 * HONEST FIX: Uses context.sidechainBuffer to drive the gain reduction (Duck).
 */
class SidechainCompressor : public IProcessor {
public:
    SidechainCompressor() : m_threshold(0.2f), m_ratio(10.0f), m_attack(10.0f), m_release(100.0f) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = sr;
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const ProcessContext& context) noexcept override {
        const uint32_t samples = buffer.getNumSamples();
        if (samples == 0 || buffer.getNumChannels() == 0) return;
        const float* scL = nullptr;
        const float* scR = nullptr;
        if (context.sidechainBuffer &&
            context.sidechainBuffer->getNumSamples() >= samples &&
            context.sidechainBuffer->getNumChannels() > 0) {
            scL = context.sidechainBuffer->getReadPointer(0);
            scR = context.sidechainBuffer->getNumChannels() > 1
                ? context.sidechainBuffer->getReadPointer(1) : scL;
        }
        const float* mainL = buffer.getReadPointer(0);
        const float* mainR = buffer.getNumChannels() > 1 ? buffer.getReadPointer(1) : mainL;
        float* outL = buffer.getWritePointer(0);
        float* outR = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : outL;
        const double sr = (std::isfinite(context.sampleRate) && context.sampleRate > 1.0)
            ? context.sampleRate : m_sampleRate;
        const float attackCoeff = 1.0f - std::exp(-1.0f /
            static_cast<float>(std::max(1.0, m_attack) * 0.001 * sr));
        const float releaseCoeff = 1.0f - std::exp(-1.0f /
            static_cast<float>(std::max(1.0, m_release) * 0.001 * sr));
        const float threshold = std::clamp(std::isfinite(m_threshold) ? m_threshold : 0.2f,
                                           1.0e-5f, 1.0f);
        const float ratio = std::max(1.0f, std::isfinite(m_ratio) ? m_ratio : 1.0f);
        for (uint32_t i = 0; i < samples; ++i) {
            const float detectorL = scL ? scL[i] : mainL[i];
            const float detectorR = scR ? scR[i] : mainR[i];
            const float detector = std::max(std::abs(detectorL), std::abs(detectorR));
            const float targetEnv = std::isfinite(detector) ? detector : 0.0f;
            const float envCoeff = targetEnv > m_env ? attackCoeff : releaseCoeff;
            m_env += (targetEnv - m_env) * envCoeff;
            float desiredGain = 1.0f;
            if (m_env > threshold) {
                const float compressed = threshold + (m_env - threshold) / ratio;
                desiredGain = std::clamp(compressed / std::max(m_env, 1.0e-6f), 0.0f, 1.0f);
            }
            const float gainCoeff = desiredGain < m_currentGain ? attackCoeff : releaseCoeff;
            m_currentGain += (desiredGain - m_currentGain) * gainCoeff;
            outL[i] = std::isfinite(mainL[i] * m_currentGain) ? mainL[i] * m_currentGain : 0.0f;
            if (outR != outL) {
                outR[i] = std::isfinite(mainR[i] * m_currentGain) ? mainR[i] * m_currentGain : 0.0f;
            }
        }
    }


    void reset() noexcept override {
        m_env = 0.0f;
        m_currentGain = 1.0f;
    }

    // Parameters
    void setThreshold(float t) { m_threshold = t; }
    void setRatio(float r) { m_ratio = r; }
    void setAttack(float ms) { m_attack = ms; }
    void setRelease(float ms) { m_release = ms; }

private:
    double m_sampleRate = 44100.0;
    float m_threshold, m_ratio, m_attack, m_release;
    float m_env = 0.0f;
    float m_currentGain = 1.0f;
};

} // namespace Aura::DSP::Effects
