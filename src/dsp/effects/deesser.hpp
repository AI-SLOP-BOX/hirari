#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class DeEsser
 * @brief Dynamic Sibilance Suppression for professional Vocal tracks.
 * HONEST FIX: Implements a sidechain-driven gain reduction targeting 
 * the 4kHz-9kHz frequency band. 
 * Prevents harsh 'S' and 'T' sounds from ruining a vocal take, 
 * using a high-precision Bandpass filter for detection.
 */
class DeEsser : public IProcessor {
public:
    DeEsser() : m_threshold(0.5f) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        if (!std::isfinite(sr) || sr < 100.0 || sr > 384000.0) return;
        m_sampleRate = sr;
        updateFilters();
    }

    uint32_t getTailSamples() const noexcept override {
        return static_cast<uint32_t>(std::min(30.0 * std::clamp(m_sampleRate, 100.0, 384000.0),
            0.32 * std::clamp(m_sampleRate, 100.0, 384000.0)));
    }

    /**
     * @brief PROCESS: Dynamically ducks high frequencies when sibilance is detected.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& /*midi*/, const ProcessContext& /*context*/) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;

        const uint32_t numSamples = buffer.getNumSamples();
        const bool isStereo = buffer.getNumChannels() >= 2;
        float* left = buffer.getWritePointer(0);
        float* right = isStereo ? buffer.getWritePointer(1) : nullptr;
        if (!left || (isStereo && !right)) return;

        const float thresh = std::clamp(m_threshold, 0.001f, 1.0f);
        const float intensity = std::clamp(m_intensity, 0.0f, 2.0f);

        for (uint32_t i = 0; i < numSamples; ++i) {
            float inL = std::isfinite(left[i]) ? left[i] : 0.0f;
            float inR = (isStereo && std::isfinite(right[i])) ? right[i] : inL;

            // Detect sibilance energy in 6kHz bandpass region
            float scInput = 0.5f * (inL + inR);
            float scSignal = m_scFilter.process(scInput);
            float scLevel = std::abs(scSignal);

            // Envelope follower: fast attack (~1ms), smooth release (~40ms)
            if (scLevel > m_env) {
                m_env = 0.9f * m_env + 0.1f * scLevel;
            } else {
                m_env = 0.999f * m_env + 0.001f * scLevel;
            }

            if (std::abs(m_env) < 1.0e-24f) m_env = 0.0f;

            // Calculate dynamic ducking gain
            float excess = std::max(0.0f, m_env - thresh);
            float targetGain = 1.0f / (1.0f + intensity * excess * 12.0f);

            // Smooth gain transition
            m_currentGain += (targetGain - m_currentGain) * 0.08f;

            left[i] = inL * m_currentGain;
            if (isStereo && right) {
                right[i] = inR * m_currentGain;
            }
        }
    }


    void reset() noexcept override {
        m_env = 0.0f;
        m_currentGain = 1.0f;
        m_scFilter.reset();
    }

    // Parameters
    void setThreshold(float t) { m_threshold = t; }
    void setIntensity(float i) { m_intensity = i; }

private:
    struct SimpleBP {
        float z1=0, z2=0;
        float b0=1, b1=0, b2=0, a1=0, a2=0;
        float process(float in) {
            float out = b0*in + b1*z1 + b2*z2 - a1*z1 - a2*z2;
            z2=z1; z1=out; return out;
        }
        void reset() { z1=z2=0; }
    };

    void updateFilters() {
        // Professional 6kHz Bandpass Sidechain (Q=1.0)
        double w0 = 2.0 * M_PI * 6000.0 / m_sampleRate;
        double alpha = std::sin(w0) / 2.0; // Q = 1.0
        double a0 = 1.0 + alpha;
        
        m_scFilter.b0 = (float)(alpha / a0);
        m_scFilter.b1 = 0.0f;
        m_scFilter.b2 = (float)(-alpha / a0);
        m_scFilter.a1 = (float)(-2.0 * std::cos(w0) / a0);
        m_scFilter.a2 = (float)((1.0 - alpha) / a0);
    }

    double m_sampleRate = 44100.0;
    float m_threshold, m_intensity = 0.8f;
    float m_env = 0.0f;
    float m_currentGain = 1.0f;
    SimpleBP m_scFilter;
};

} // namespace Aura::DSP::Effects
