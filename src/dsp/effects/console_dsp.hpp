/* Aura DAW Ultimate - Divine Console (Master Buss Core) - (c) 2026 Aura DAW Project */
#pragma once
#include "../../core/audio_buffer.hpp"
#include <cmath>

namespace Aura::DSP::Effects {

/**
 * @class DivineConsole
 * @brief SSL-Inspired Master Buss Processor with VCA Compression and Harmonic Saturation.
 */
class DivineConsole {
public:
    DivineConsole() {
        m_threshold = 0.5f; // -6dB ballpark
        m_ratio = 4.0f;
        m_attack = 0.01f;
        m_release = 0.1f;
        m_gainEnv = 1.0f;
    }

    void prepareToPlay(double sr, uint32_t /*maxBlockSize*/) {
        if (std::isfinite(sr) && sr > 0.0) m_sr = sr;
        m_gainEnv = 1.0f;
    }

    void process(::Aura::Core::AudioBuffer& buffer, uint32_t samples, const float* sidechain = nullptr) {
        const uint32_t count = std::min(samples, buffer.getNumSamples());
        const uint32_t channels = buffer.getNumChannels();
        if (count == 0 || channels == 0) return;

        const float threshold = std::clamp(std::isfinite(m_threshold) ? m_threshold : 0.5f,
                                           1.0e-5f, 1.0f);
        const float ratio = std::clamp(std::isfinite(m_ratio) ? m_ratio : 4.0f, 1.0f, 20.0f);
        const float drive = std::clamp(std::isfinite(m_drive) ? m_drive : 1.0f, 1.0f, 8.0f);
        const float attack = std::clamp(std::isfinite(m_attack) ? m_attack : 0.01f, 1.0e-5f, 2.0f);
        const float release = std::clamp(std::isfinite(m_release) ? m_release : 0.1f, 1.0e-5f, 4.0f);
        const float attackCoeff = std::exp(-1.0f / static_cast<float>(m_sr * attack));
        const float releaseCoeff = std::exp(-1.0f / static_cast<float>(m_sr * release));
        const float saturationNorm = std::tanh(drive);

        if (!std::isfinite(m_gainEnv)) m_gainEnv = 1.0f;
        for (uint32_t i = 0; i < count; ++i) {
            float detector = 0.0f;
            if (sidechain != nullptr && std::isfinite(sidechain[i])) {
                detector = std::abs(sidechain[i]);
            } else {
                for (uint32_t channel = 0; channel < channels; ++channel) {
                    const float value = buffer.getReadPointer(channel)[i];
                    if (std::isfinite(value)) detector = std::max(detector, std::abs(value));
                }
            }

            const float targetGain = detector > threshold
                ? std::pow(threshold / detector, 1.0f - 1.0f / ratio)
                : 1.0f;
            const float coeff = targetGain < m_gainEnv ? attackCoeff : releaseCoeff;
            m_gainEnv += (targetGain - m_gainEnv) * (1.0f - coeff);
            m_gainEnv = std::clamp(std::isfinite(m_gainEnv) ? m_gainEnv : 1.0f, 0.0f, 1.0f);

            for (uint32_t channel = 0; channel < channels; ++channel) {
                float* output = buffer.getWritePointer(channel);
                const float input = std::isfinite(output[i]) ? std::clamp(output[i], -8.0f, 8.0f) : 0.0f;
                const float saturated = std::tanh(input * drive) / saturationNorm;
                output[i] = std::isfinite(saturated) ? saturated * m_gainEnv : 0.0f;
            }
        }
    }


    void setDrive(float drive) {
        m_drive = 1.0f + (std::isfinite(drive) ? std::clamp(drive, 0.0f, 7.0f) : 0.0f);
    }

    void setParams(float thresholddB, float ratio) {
        const float db = std::isfinite(thresholddB) ? std::clamp(thresholddB, -100.0f, 0.0f) : -6.0f;
        m_threshold = std::pow(10.0f, db / 20.0f);
        m_ratio = std::isfinite(ratio) ? std::clamp(ratio, 1.0f, 20.0f) : 4.0f;
    }

private:
    double m_sr = 44100.0;
    float m_threshold, m_ratio, m_attack, m_release;
    float m_gainEnv;
    float m_drive = 1.0f;
};

} // namespace Aura::DSP::Effects
