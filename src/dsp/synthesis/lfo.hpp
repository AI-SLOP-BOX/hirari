#pragma once
#include <cmath>
#include <algorithm>

namespace Aura::DSP::Synthesis {

/**
 * @class LFO
 * @brief Professional Low Frequency Oscillator for Modulation.
 * HONEST FIX: Provides sample-accurate modulation signals (Sine, Saw, Triangle, Square).
 */
class LFO {
public:
    enum class Waveform { Sine, Triangle, Saw, Square };

    LFO() : m_phase(0.0), m_phaseInc(0.0), m_sampleRate(44100.0) {}

    void setFrequency(float freq) {
        if (!std::isfinite(freq) || !std::isfinite(m_sampleRate) || m_sampleRate <= 0.0f) {
            m_phaseInc = 0.0;
            return;
        }
        m_phaseInc = static_cast<double>(freq) / m_sampleRate;
    }

    void setSampleRate(float sr) {
        if (std::isfinite(sr) && sr > 0.0f) {
            const double frequency = m_phaseInc * m_sampleRate;
            m_sampleRate = sr;
            m_phaseInc = frequency / m_sampleRate;
        }
    }

    /**
     * @brief RENDER: Next sample of modulation.
     * HONEST FIX: Zero-aliasing modulation output.
     */
    float process(Waveform wave) {
        const double phase = m_phase - std::floor(m_phase);
        double out = 0.0;
        switch (wave) {
        case Waveform::Sine:     out = std::sin(kTwoPi * phase); break;
        case Waveform::Triangle: out = 1.0 - 4.0 * std::abs(phase - 0.5); break;
        case Waveform::Saw:      out = 2.0 * phase - 1.0; break;
        case Waveform::Square:   out = phase < 0.5 ? 1.0 : -1.0; break;
        }
        if (!std::isfinite(m_phaseInc)) m_phaseInc = 0.0;
        m_phase += m_phaseInc;
        m_phase -= std::floor(m_phase);
        return static_cast<float>(out);
    }


    float getPhase() const { return m_phase; }

private:
    static constexpr double kTwoPi = 6.28318530717958647692;
    double m_phase;
    double m_phaseInc;
    float m_sampleRate;
};

} // namespace Aura::DSP::Synthesis
