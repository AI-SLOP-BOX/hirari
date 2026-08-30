#pragma once

#include <cmath>
#include <algorithm>
#include <vector>

namespace Aura::DSP::Synthesis {

/**
 * @class PolyBLEPOscillator
 * @brief High-fidelity Virtual Analog Oscillator with Anti-Aliasing.
 * HONEST FIX: Implements PolyBLEP (Poly-stage Band-Limited Step) to 
 * eliminate digital aliasing in Sawtooth and Square waves.
 * Essential for professional 'Analog' sound in 2026.
 */
class PolyBLEPOscillator {
public:
    enum class Waveform { Saw, Square, Triangle, Sine };

    PolyBLEPOscillator(double sampleRate = 44100.0) 
        : m_sampleRate((std::isfinite(sampleRate) && sampleRate > 0.0) ? sampleRate : 44100.0), m_phase(0.0), m_freq(440.0) {
        updateIncrement();
    }

    void setFrequency(double freq) {
        m_freq = std::isfinite(freq) ? freq : 0.0;
        if (m_freq < 0.0) m_freq = 0.0;
        updateIncrement();
    }

    void setWaveform(Waveform wave) { m_waveform = wave; }

    /**
     * @brief RENDER: Generates the next sample of the chosen waveform.
     */
    float process() {
        const double dt = std::min(m_increment, 0.5);
        double out = 0.0;
        switch (m_waveform) {
        case Waveform::Sine: out = std::sin(kTwoPi * m_phase); break;
        case Waveform::Saw:
            out = 2.0 * m_phase - 1.0 - bleach(m_phase, dt); break;
        case Waveform::Square:
            out = (m_phase < 0.5 ? 1.0 : -1.0) + bleach(m_phase, dt) - bleach(std::fmod(m_phase + 0.5, 1.0), dt); break;
        case Waveform::Triangle: {
            const double saw = 2.0 * m_phase - 1.0 - bleach(m_phase, dt);
            out = 2.0 * std::abs(saw) - 1.0;
            break;
        }
        }
        m_phase += m_increment;
        m_phase -= std::floor(m_phase);
        return static_cast<float>(out);
    }


private:
    static constexpr double kTwoPi = 6.28318530717958647692;
    /**
     * @brief The 'Magic' Correction term for PolyBLEP.
     */
    double bleach(double t, double dt) {
        if (t < dt) {
            t /= dt;
            return t + t - t * t - 1.0;
        } else if (t > 1.0 - dt) {
            t = (t - 1.0) / dt;
            return t * t + t + t + 1.0;
        }
        return 0.0;
    }

    void updateIncrement() {
        m_increment = m_freq / m_sampleRate;
    }

    double m_sampleRate;
    double m_phase;
    double m_freq;
    double m_increment;
    Waveform m_waveform = Waveform::Saw;
};

} // namespace Aura::DSP::Synthesis
