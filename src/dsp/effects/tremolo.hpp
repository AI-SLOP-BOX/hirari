#pragma once

#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class StereoTremolo
 * @brief High-end Volume and Pan Modulation (Rhodes style).
 * HONEST FIX: Implements synchronized amplitude modulation (AM) 
 * with a phase-offset between Left and Right to create the classic 
 * 'Auto-Pan' movement found in vintage electric pianos.
 */
class StereoTremolo : public IProcessor {
public:
    StereoTremolo() : m_lfoPhase(0.0), m_depth(0.0), m_stereoWidth(0.0) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = sr;
    }

    /**
     * @brief PROCESS: Rhythmic volume and pan modulation.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        const uint32_t n = buffer.getNumSamples();
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!left || n == 0) return;
        const double sr = m_sampleRate > 1000.0 ? m_sampleRate : 44100.0;
        const double bpm = std::isfinite(context.bpm) && context.bpm > 1.0 ? context.bpm : 120.0;
        const double hz = bpm / (60.0 * std::max(0.0625, static_cast<double>(m_noteValue)));
        const double inc = hz / sr;
        for (uint32_t i = 0; i < n; ++i) {
            const float lfo = static_cast<float>(0.5 + 0.5 * std::sin(2.0 * M_PI * m_lfoPhase));
            const float amplitude = 1.0f - m_depth * (1.0f - lfo);
            const float pan = m_stereoWidth * std::sin(2.0 * M_PI * m_lfoPhase);
            const float gainL = amplitude * (1.0f - 0.25f * pan);
            const float gainR = amplitude * (1.0f + 0.25f * pan);
            left[i] *= gainL;
            if (right) right[i] *= gainR;
            m_lfoPhase += inc;
            if (m_lfoPhase >= 1.0) m_lfoPhase -= std::floor(m_lfoPhase);
        }
    }


    void reset() noexcept override {
        m_lfoPhase = 0.0;
    }

    // Parameters
    void setDepth(float d) { m_depth = std::clamp(d, 0.0f, 1.0f); }
    void setNoteValue(float v) { m_noteValue = v; } // 0.25 (Quarter), 0.5 (Half), etc.
    void setStereoWidth(float w) { m_stereoWidth = std::clamp(w, 0.0f, 1.0f); }

private:
    double m_sampleRate = 44100.0;
    double m_lfoPhase;
    float m_depth;
    float m_noteValue = 0.25f;
    float m_stereoWidth = 0.5f; // 0.5 = 180 deg (Full Pan)
};

} // namespace Aura::DSP::Effects
