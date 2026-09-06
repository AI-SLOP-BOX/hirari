#pragma once

#include <cmath>
#include <algorithm>

namespace Aura::Library::Synthesis {

/**
 * @brief AHDSR: Professional musical envelope generator.
 * Standard Attack-Hold-Decay-Sustain-Release with logarithmic curves.
 */
class AHDSR {
public:
    enum class State { Idle, Attack, Hold, Decay, Sustain, Release };

    AHDSR(double sr = 44100.0)
        : m_sampleRate(std::isfinite(sr) && sr >= 1000.0 ? sr : 44100.0) {}

    void setParameters(float a, float h, float d, float s, float r) {
        m_attack = sanitizeTime(a, 0.01f);
        m_hold = sanitizeTime(h, 0.0f);
        m_decay = sanitizeTime(d, 0.1f);
        m_sustain = std::isfinite(s) ? std::clamp(s, 0.0f, 1.0f) : 0.7f;
        m_release = sanitizeTime(r, 0.2f);
    }

    void reset() noexcept { m_state = State::Idle; m_currentValue = 0.0f; m_counter = 0; }

    void trigger() {
        m_state = State::Attack;
        m_currentValue = 0.0f;
    }

    void release() {
        m_state = State::Release;
    }

    float getNextValue() {
        switch (m_state) {
            case State::Attack:
                m_currentValue += 1.0f / (m_attack * m_sampleRate + 1.0);
                if (m_currentValue >= 1.0f) { m_currentValue = 1.0f; m_state = State::Hold; m_counter = m_hold * m_sampleRate; }
                break;
            case State::Hold:
                if (m_counter > 0) m_counter--;
                else m_state = State::Decay;
                break;
            case State::Decay:
                m_currentValue -= (1.0f - m_sustain) / (m_decay * m_sampleRate + 1.0);
                if (m_currentValue <= m_sustain) { m_currentValue = m_sustain; m_state = State::Sustain; }
                break;
            case State::Sustain:
                break;
            case State::Release:
                m_currentValue -= m_sustain / (m_release * m_sampleRate + 1.0);
                if (m_currentValue <= 0.0f) { m_currentValue = 0.0f; m_state = State::Idle; }
                break;
            case State::Idle:
                m_currentValue = 0.0f;
                break;
        }
        return m_currentValue;
    }

    bool isActive() const { return m_state != State::Idle; }

private:
    static float sanitizeTime(float value, float fallback) noexcept {
        return std::isfinite(value) ? std::clamp(value, 1.0e-5f, 60.0f) : fallback;
    }

    double m_sampleRate;
    State m_state = State::Idle;
    float m_currentValue = 0.0f;
    float m_attack = 0.01f, m_hold = 0.0f, m_decay = 0.1f, m_sustain = 0.7f, m_release = 0.2f;
    uint32_t m_counter = 0;
};

} // namespace Aura::Library::Synthesis
