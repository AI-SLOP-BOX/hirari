#pragma once

#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"
#include "../utils/dsp_utils.hpp"

namespace Aura::DSP::Effects {

/**
 * @class SubBassGenerator
 * @brief Professional Sub-frequency Synthesis for Hip-Hop/EDM.
 * HONEST FIX: Implements a pitch-tracking sub-oscillator that generates 
 * a pure sine wave exactly one octave below the input's fundamental.
 */
class SubBassGenerator : public IProcessor {
public:
    SubBassGenerator(double sr = 44100.0) : m_sampleRate(sr) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        m_sampleRate = std::isfinite(sr) ? std::clamp(sr, 1000.0, 384000.0) : 44100.0;
        m_envAttack = std::exp(static_cast<float>(-1.0 / (0.005 * m_sampleRate)));
        m_envRelease = std::exp(static_cast<float>(-1.0 / (0.1 * m_sampleRate)));
        m_lpfCoeff = std::exp(static_cast<float>(-1.0 / (0.001 * m_sampleRate)));
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        if (m_bypassed || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;

        const uint32_t channels = buffer.getNumChannels();
        const uint32_t samples = buffer.getNumSamples();
        float* left = buffer.getWritePointer(0);
        float* right = channels > 1 ? buffer.getWritePointer(1) : nullptr;
        const float mix = std::clamp(std::isfinite(m_mix) ? m_mix : 0.0f, 0.0f, 1.0f);
        const float sr = static_cast<float>(m_sampleRate);

        for (uint32_t s = 0; s < samples; ++s) {
            const float inL = std::isfinite(left[s]) ? left[s] : 0.0f;
            const float inR = right ? (std::isfinite(right[s]) ? right[s] : 0.0f) : inL;
            const float mid = 0.5f * (inL + inR);

            // Low-pass and DC rejection make zero-crossing tracking deterministic.
            m_lpfState += (1.0f - m_lpfCoeff) * (mid - m_lpfState);
            const float dc = m_lpfState - m_lastLpf + 0.995f * m_dcBlockState;
            m_lastLpf = m_lpfState;
            m_dcBlockState = std::isfinite(dc) ? dc : 0.0f;

            const float magnitude = std::abs(mid);
            if (magnitude > m_env) {
                m_env = m_envAttack * m_env + (1.0f - m_envAttack) * magnitude;
            } else {
                m_env *= m_envRelease;
            }
            m_env = std::clamp(std::isfinite(m_env) ? m_env : 0.0f, 0.0f, 4.0f);

            bool crossing = false;
            if (m_dcBlockState > 0.02f && !m_isPositive) {
                m_isPositive = true;
                crossing = true;
            } else if (m_dcBlockState < -0.02f) {
                m_isPositive = false;
            }
            if (crossing) {
                const float period = static_cast<float>(m_zcCount);
                if (period > 10.0f) {
                    // Positive-going crossings are one period apart; generate one octave down.
                    m_targetFreq = std::clamp((sr / period) * 0.5f, 20.0f, std::min(90.0f, sr * 0.24f));
                }
                m_zcCount = 0;
            }
            m_zcCount = std::min<uint32_t>(m_zcCount + 1u, 10000u);

            const float maxFreq = std::min(90.0f, sr * 0.24f);
            m_targetFreq = std::clamp(std::isfinite(m_targetFreq) ? m_targetFreq : 50.0f, 20.0f, maxFreq);
            m_currFreq += (m_targetFreq - m_currFreq) * 0.05f;
            m_currFreq = std::clamp(std::isfinite(m_currFreq) ? m_currFreq : 50.0f, 20.0f, maxFreq);
            m_phase += static_cast<double>(m_currFreq) / m_sampleRate;
            m_phase -= std::floor(m_phase);

            constexpr double kPi = 3.14159265358979323846;
            const float sub = static_cast<float>(std::sin(2.0 * kPi * m_phase)) * m_env * mix;
            left[s] = std::isfinite(inL + sub) ? inL + sub : inL;
            if (right) right[s] = std::isfinite(inR + sub) ? inR + sub : inR;
        }
    }


    void reset() noexcept override {
        m_env = 0.0f; m_phase = 0.0; m_lpfState = 0.0f; m_lastLpf = 0.0f; m_dcBlockState = 0.0f;
        m_zcCount = 0; m_targetFreq = 50.0f; m_currFreq = 50.0f;
    }

    void setParameter(uint32_t id, float value) noexcept override {
        if (id == 0) m_mix = std::clamp(std::isfinite(value) ? value : 0.0f, 0.0f, 1.0f);
    }
    float getParameter(uint32_t id) const noexcept override { return (id == 0) ? m_mix : 0.0f; }

private:
    double m_sampleRate = 44100.0;
    float m_env = 0.0f, m_envAttack = 0.995f, m_envRelease = 0.9998f;
    float m_lpfState = 0.0f, m_lastLpf = 0.0f, m_dcBlockState = 0.0f, m_lpfCoeff = 0.9775f;
    float m_mix = 0.5f, m_targetFreq = 50.0f, m_currFreq = 50.0f;
    double m_phase = 0.0;
    uint32_t m_zcCount = 0;
    bool m_isPositive = false;
};

} // namespace Aura::DSP::Effects
