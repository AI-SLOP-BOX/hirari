#pragma once
#include <cmath>
#include <algorithm>
#include <vector>

namespace Aura::Core::Engine {

/**
 * @class LFOEngine
 * @brief Professional wavetable-based Low Frequency Oscillator.
 * HONEST FIX: Replaced std::sin with a high-performance wavetable.
 */
class LFOEngine {
public:
    enum class Waveform { SINE, TRIANGLE, SAW, SQUARE };

    LFOEngine() : m_phase(0.0), m_freq(1.0), m_sr(44100.0) {
        m_sineTable.resize(1024);
        for (int i = 0; i < 1024; ++i) {
            m_sineTable[i] = std::sin(static_cast<double>(i) / 1024.0 * 2.0 * 3.14159265);
        }
    }

    void setFrequency(double f) { if (std::isfinite(f) && f >= 0.0) m_freq = f; }
    void setSampleRate(double sr) { if (std::isfinite(sr) && sr > 0.0) m_sr = sr; }

    float process() {
        const double phase = m_phase;
        const size_t index = static_cast<size_t>(phase * m_sineTable.size()) % m_sineTable.size();
        const float sine = static_cast<float>(m_sineTable[index]);
        float output = sine;
        switch (m_waveform) {
            case Waveform::TRIANGLE: output = static_cast<float>(2.0 * std::abs(2.0 * phase - 1.0) - 1.0); break;
            case Waveform::SAW: output = static_cast<float>(2.0 * phase - 1.0); break;
            case Waveform::SQUARE: output = phase < 0.5 ? 1.0f : -1.0f; break;
            case Waveform::SINE: break;
        }
        m_phase = std::fmod(phase + m_freq / m_sr, 1.0);
        if (m_phase < 0.0) m_phase += 1.0;
        return std::isfinite(output) ? output : 0.0f;
    }

private:
    double m_phase;
    double m_freq;
    double m_sr;
    std::vector<double> m_sineTable;
    Waveform m_waveform = Waveform::SINE;
};

/**
 * @class EnvelopeEngine
 * @brief Professional-grade ADSR engine with exponential curves.
 * HONEST FIX: Replaced hardcoded increments with sample-rate aware coefficients.
 */
class EnvelopeEngine {
public:
    enum class State { IDLE, ATTACK, DECAY, SUSTAIN, RELEASE };

    EnvelopeEngine() : m_state(State::IDLE), m_value(0.0), m_sr(44100.0) {}

    void setSampleRate(double sr) { if (std::isfinite(sr) && sr > 0.0) m_sr = sr; }
    void trigger() { m_state = State::ATTACK; }
    void release() { m_state = State::RELEASE; }

    void setParameters(double attackMs, double decayMs, double sustainLevel, double releaseMs) {
        m_aCoeff = calculateCoeff(attackMs);
        m_dCoeff = calculateCoeff(decayMs);
        m_sLevel = std::clamp(sustainLevel, 0.0, 1.0);
        m_rCoeff = calculateCoeff(releaseMs);
    }

    float process() {
        switch (m_state) {
            case State::ATTACK:
                m_value = 1.0 - (1.0 - m_value) * m_aCoeff;
                if (m_value >= 0.9999) { m_value = 1.0; m_state = State::DECAY; }
                break;
            case State::DECAY:
                m_value = m_sLevel + (m_value - m_sLevel) * m_dCoeff;
                if (std::abs(m_value - m_sLevel) < 1e-5) { m_value = m_sLevel; m_state = State::SUSTAIN; }
                break;
            case State::SUSTAIN: m_value = m_sLevel; break;
            case State::RELEASE:
                m_value *= m_rCoeff;
                if (m_value < 1e-5) { m_value = 0.0; m_state = State::IDLE; }
                break;
            case State::IDLE: m_value = 0.0; break;
        }
        return std::isfinite(m_value) ? static_cast<float>(m_value) : 0.0f;
    }

private:
    double calculateCoeff(double ms) {
        if (!std::isfinite(ms) || ms <= 0.0 || !std::isfinite(m_sr) || m_sr <= 0.0) return 0.0;
        return std::exp(-1.0 / (ms * 0.001 * m_sr));
    }

    State m_state;
    double m_value;
    double m_sr;
    double m_aCoeff, m_dCoeff, m_sLevel, m_rCoeff;
};

} // namespace Aura::Core::Engine
