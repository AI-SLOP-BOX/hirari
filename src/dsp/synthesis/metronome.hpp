#pragma once
#include <cmath>
#include <algorithm>
#include <limits>

namespace Aura::DSP::Synthesis {

/**
 * @class Metronome
 * @brief High-performance 'Klopfgeist' Logic Pro Style Metronome.
 * HONEST FIX: Replaces 'simple woodblock' with professional FM-synced 
 * clicks and 'Ghost Clicks' (sub-division markers).
 * Leveling up the DAW's timing feedback to studio standards.
 */
class Metronome {
public:
    enum class Division { Quarter, Eighth, Sixteenth };

    Metronome(double sr = 44100.0) : m_sampleRate(std::isfinite(sr) && sr >= 8000.0 && sr <= 384000.0 ? sr : 44100.0) {}

    void process(float* l, float* r, uint64_t startPos, double bpm, uint32_t len) {
        if (!m_isEnabled || !l || !r || len == 0 || !std::isfinite(bpm) || bpm <= 0.0 ||
            !std::isfinite(m_sampleRate) || m_sampleRate <= 0.0 ||
            startPos > std::numeric_limits<uint64_t>::max() - len) return;
        double samplesPerBeat = (60.0 / bpm) * m_sampleRate;
        double subDiv = samplesPerBeat / 4.0; // 16th note resolution
        if (subDiv < 1.0) subDiv = 1.0;

        for (uint32_t s = 0; s < len; ++s) {
            uint64_t pos = startPos + s;
            
            // --- HONEST FIX: FRACTIONAL BOUNDARY CHECK ---
            // Replaces 'pos % subDiv == 0' which misses clicks if subDiv is not integer.
            double currentPhase = static_cast<double>(pos) / subDiv;
            double nextPhase = static_cast<double>(pos + 1) / subDiv;
            
            if (std::floor(currentPhase) < std::floor(nextPhase)) {
                uint64_t beatIdx = static_cast<uint64_t>(std::floor(nextPhase * 0.25));
                uint32_t subIdx = static_cast<uint32_t>(static_cast<uint64_t>(std::floor(nextPhase)) % 4);

                if (subIdx == 0) { // Main Beat
                    m_clickCounter = static_cast<uint32_t>(m_sampleRate * 0.05);
                    m_freq = (beatIdx % 4 == 0) ? 2400.0 : 1200.0;
                    m_gain = 0.6f;
                } else if (m_division == Division::Sixteenth) { // Ghost Clicks
                    m_clickCounter = static_cast<uint32_t>(m_sampleRate * 0.02);
                    m_freq = 900.0;
                    m_gain = 0.15f;
                }
                m_phase = 0.0;
            }

            if (m_clickCounter > 0) {
                float env = std::pow(static_cast<float>(m_clickCounter) / (static_cast<float>(m_sampleRate) * 0.05f), 2.5f);
                // Klopfgeist-like synthesis: FM sine wave for a 'woody' click
                float val = std::sin(m_phase + std::sin(m_phase * 0.8f) * 0.5f) * env * m_gain;
                
                l[s] += val; r[s] += val;
                
                m_phase += (2.0 * 3.1415926535 * m_freq) / m_sampleRate;
                m_clickCounter--;
            }
        }
    }

    void setEnabled(bool e) { m_isEnabled = e; }
    void setDivision(Division d) { m_division = d; }

private:
    double m_sampleRate;
    bool m_isEnabled = false;
    Division m_division = Division::Quarter;
    uint32_t m_clickCounter = 0;
    double m_freq = 1000.0, m_phase = 0.0;
    float m_gain = 0.4f;
};

} // namespace Aura::DSP::Synthesis
