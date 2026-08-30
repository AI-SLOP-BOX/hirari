#pragma once
#include <cmath>
#include <algorithm>
#include <array>
#include "../../core/audio_buffer.hpp"

namespace Aura::DSP::Effects {

/**
 * @class StateVariableFilter
 * @brief Professional Zero-Delay Feedback (ZDF) SVF.
 * Support for Block-processing and legendary Pultec-style shelf extensions.
 */
class StateVariableFilter {
public:
    enum Type {
        LowPass, HighPass, BandPass, Bell, Notch, LowShelf, HighShelf
    };

    StateVariableFilter(double sr = 44100.0) : m_sampleRate(sr) {
        reset();
    }

    void setType(Type t) { m_type = t; }
    void prepareToPlay(double sr, uint32_t bs) { m_sampleRate = sr; }
    
    /**
     * @brief SET PARAMS: SVF Logic for Analog Modeling.
     */
    void setParams(float freq, float gainDb, float Q) {
        freq = std::clamp(freq, 5.0f, static_cast<float>(m_sampleRate * 0.45));
        Q = std::clamp(Q, 0.05f, 20.0f);
        float g = std::tan(static_cast<float>(M_PI) * freq / static_cast<float>(m_sampleRate));
        float k = 1.0f / Q;
        float a = std::pow(10.0f, gainDb / 40.0f); // Half-gain for shelf sum logic
        
        m_g = g; m_k = k; m_gain = a;
        m_a1 = 1.0f / (1.0f + g * (g + k));
        m_a2 = g * m_a1;
        m_a3 = g * m_a2;
    }

    /**
     * @brief BLOCK PROCESS: High-performance buffer sum.
     */
    void process(Core::AudioBuffer& buffer) {
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        for (uint32_t c = 0; c < channels; ++c) {
            float* data = buffer.getWritePointer(c);
            for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
                const float input = std::isfinite(data[i]) ? data[i] : 0.0f;
                const float v3 = input - m_s2[c];
                const float v1 = m_a1 * m_s1[c] + m_a2 * v3;
                const float v2 = m_s2[c] + m_a2 * m_s1[c] + m_a3 * v3;
                m_s1[c] = 2.0f * v1 - m_s1[c];
                m_s2[c] = 2.0f * v2 - m_s2[c];
                float output = 0.0f;
                switch (m_type) {
                    case LowPass: output = v2; break;
                    case HighPass: output = input - m_k * v1 - v2; break;
                    case BandPass: output = v1; break;
                    case Notch: output = input - m_k * v1; break;
                    case Bell: output = input + (m_gain - 1.0f) * v1; break;
                    case LowShelf: output = input + (m_gain - 1.0f) * v2; break;
                    case HighShelf: output = input + (m_gain - 1.0f) * (input - v2); break;
                }
                data[i] = std::isfinite(output) ? output : 0.0f;
            }
        }
    }


    void reset() {
        m_s1.fill(0.0f);
        m_s2.fill(0.0f);
    }

private:
    double m_sampleRate;
    Type m_type = LowPass;
    float m_g, m_k, m_gain, m_a1, m_a2, m_a3;
    std::array<float, 2> m_s1, m_s2; // Max stereo
};

} // namespace Aura::DSP::Effects
