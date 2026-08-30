#pragma once

#include <cmath>
#include <atomic>
#include <algorithm>

namespace Aura::Core::DSP::Synthesis {

/**
 * @class SubtractiveSynth
 * @brief Real Subtractive Synthesizer Engine.
 * Generates a raw sawtooth wave, applies a sweeping low-pass filter, and scales with a decay envelope.
 */
class SubtractiveSynth {
public:
    explicit SubtractiveSynth(double sr) : m_sampleRate(sr), m_phase(0.0f), m_noteAge(0.0f), m_lowpassState(0.0f) {}

    /**
     * @brief Triggers a note with given frequency and velocity.
     */
    void noteOn(float frequency, float velocity) {
        m_frequency.store(frequency);
        m_velocity.store(velocity);
        m_phase = 0.0f;
        m_noteAge = 0.0f;
        m_lowpassState = 0.0f;
        m_isActive.store(true);
    }

    /**
     * @brief Renders one block of subtractive synthesis audio.
     */
    void render(float* l, float* r, size_t numFrames) {
        if (!m_isActive.load()) return;

        float freq = m_frequency.load();
        float vel = m_velocity.load();
        float dt = 1.0f / static_cast<float>(m_sampleRate);

        for (size_t i = 0; i < numFrames; ++i) {
            // 1. Raw Sawtooth Wave Oscillator
            float raw = 2.0f * m_phase - 1.0f;
            
            // 2. Filter Envelope: Cutoff sweep from high to low over time
            float cutoffEnv = 0.1f + 0.7f * std::exp(-m_noteAge * 4.0f);
            
            // 3. Subtractive One-pole Low-Pass Filter
            m_lowpassState += cutoffEnv * (raw - m_lowpassState);
            
            // 4. Amplitude Decay Envelope
            float ampEnv = std::exp(-m_noteAge * 1.5f);
            float sample = m_lowpassState * ampEnv * vel * 0.5f;

            l[i] += sample;
            r[i] += sample;

            // Phase and age progression
            m_phase += freq / m_sampleRate;
            if (m_phase >= 1.0f) m_phase -= 1.0f;

            m_noteAge += dt;
            if (ampEnv < 0.001f) {
                m_isActive.store(false);
                break;
            }
        }
    }

private:
    double m_sampleRate;
    std::atomic<bool> m_isActive{false};
    std::atomic<float> m_frequency{440.0f};
    std::atomic<float> m_velocity{0.0f};
    float m_phase;
    float m_noteAge;
    float m_lowpassState;
};

} // namespace Aura::Core::DSP::Synthesis
