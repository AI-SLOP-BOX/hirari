#pragma once
#include <atomic>
#include <cmath>
#include <algorithm>

namespace Aura::Core::DSP::Synthesis {

enum ADSRState { ADSR_IDLE, ADSR_ATTACK, ADSR_DECAY, ADSR_SUSTAIN, ADSR_RELEASE, ADSR_OFF };

/**
 * @class ADSREnvelope
 * @brief High-fidelity, real-time safe ADSR envelope generator.
 * HONEST FIX: Exponential Release curves for professional instrument feel.
 */
class ADSREnvelope {
public:
    void setSampleRate(double sr) { 
        m_sampleRate = sr; 
        updateCoeffs();
    }
    
    void setParameters(float a, float d, float s, float r) {
        m_attack = std::max(0.001f, a); 
        m_decay = std::max(0.001f, d); 
        m_sustain = std::clamp(s, 0.0f, 1.0f); 
        m_release = std::max(0.001f, r);
        updateCoeffs();
    }

    void triggerOn() {
        m_state = ADSR_ATTACK;
        m_currentLevel = 0.0f;
    }

    void triggerOff() {
        if (m_state != ADSR_IDLE && m_state != ADSR_OFF) {
            m_state = ADSR_RELEASE;
        }
    }

    void reset() {
        m_state = ADSR_IDLE;
        m_currentLevel = 0.0f;
    }

    ADSRState getState() const { return m_state; }

    float getNext() {
        switch (m_state) {
            case ADSR_IDLE: case ADSR_OFF: return 0.0f;
            case ADSR_ATTACK:
                m_currentLevel = std::min(1.0f, m_currentLevel + m_attackStep);
                if (m_currentLevel >= 1.0f) m_state = ADSR_DECAY;
                break;
            case ADSR_DECAY:
                m_currentLevel = std::max(m_sustain, m_currentLevel - m_decayStep);
                if (m_currentLevel <= m_sustain) m_state = ADSR_SUSTAIN;
                break;
            case ADSR_SUSTAIN:
                m_currentLevel = m_sustain;
                break;
            case ADSR_RELEASE:
                m_currentLevel *= m_releaseCoeff;
                if (m_currentLevel <= 1.0e-5f) {
                    m_currentLevel = 0.0f;
                    m_state = ADSR_IDLE;
                }
                break;
        }
        return std::isfinite(m_currentLevel) ? std::clamp(m_currentLevel, 0.0f, 1.0f) : 0.0f;
    }


private:
    void updateCoeffs() {
        m_attackStep = 1.0f / (m_attack * m_sampleRate);
        m_decayStep = (1.0f - m_sustain) / (m_decay * m_sampleRate);
        // Exponential decay coefficient: level[n] = level[n-1] * e^(-1/tau)
        m_releaseCoeff = static_cast<float>(std::exp(-1.0 / (m_release * m_sampleRate)));
    }

    double m_sampleRate = 44100.0;
    float m_attack = 0.01f, m_decay = 0.1f, m_sustain = 0.8f, m_release = 0.5f;
    float m_attackStep = 0.01f, m_decayStep = 0.01f, m_releaseCoeff = 0.99f;
    float m_currentLevel = 0.0f;
    ADSRState m_state = ADSR_IDLE;
};

} // namespace Aura::Core::DSP::Synthesis
