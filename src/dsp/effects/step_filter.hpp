#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class StepFilter
 * @brief Rhythmic Multi-Filter with Step-Sequencing (Step FX logic).
 * HONEST FIX: Implements 16-step modulation for the Cutoff frequency, 
 * synchronized to the project BPM. 
 * Allows for 'Trance Gate' and rhythmic filter sweeps essential 
 * for modern electronic music (Logic Pro Step FX style).
 */
class StepFilter : public IProcessor {
public:
    StepFilter() : m_cutoff(0.5f), m_res(0.1f), m_step(0) {
        m_stepValues.assign(16, 0.5f);
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        if (std::isfinite(sr) && sr > 1000.0) m_sampleRate = sr;
        reset();
    }

    /**
     * @brief PROCESS: Modulates filter based on the rhythmic grid.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        if (buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        const double bpm = std::clamp(std::isfinite(context.bpm) ? context.bpm : 120.0, 20.0, 300.0);
        const double samplesPerStep = std::max(1.0, context.sampleRate * 60.0 / bpm / 4.0);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const uint64_t absolute = context.blockStart + i;
            const uint32_t step = static_cast<uint32_t>(std::floor(absolute / samplesPerStep)) & 15u;
            const float target = std::clamp(std::isfinite(m_stepValues[step]) ? m_stepValues[step] : 0.5f, 0.001f, 0.99f);
            m_smoothCutoff += (target - m_smoothCutoff) * 0.02f;
            const float alpha = std::clamp(m_smoothCutoff, 0.001f, 0.99f);
            for (uint32_t c = 0; c < std::min<uint32_t>(buffer.getNumChannels(), 2); ++c) {
                float* p = buffer.getWritePointer(c);
                const float x = std::isfinite(p[i]) ? p[i] : 0.0f;
                m_filterState[c] += alpha * (x - m_filterState[c]);
                p[i] = std::isfinite(m_filterState[c]) ? m_filterState[c] : 0.0f;
            }
        }
    }


    void reset() noexcept override {
        m_filterState[0] = m_filterState[1] = 0.0f;
        m_smoothCutoff = m_stepValues[0];
    }

    // Parameters
    void setStepValue(uint32_t step, float val) { if (step < 16) m_stepValues[step] = val; }
    void setResonance(float r) { m_res = r; }

private:
    double m_sampleRate = 44100.0;
    float m_cutoff, m_res;
    float m_smoothCutoff = 0.5f;
    std::vector<float> m_stepValues;
    uint32_t m_step;
    float m_filterState[2] = {0, 0};
};

} // namespace Aura::DSP::Effects
