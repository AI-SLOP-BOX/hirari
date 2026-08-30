#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"
#include "../../core/engine/bus_system.hpp"

namespace Aura::DSP::Effects {

/**
 * @class SidechainDuck
 * @brief Dynamic Pumping effect for professional Electronic/Trap music.
 * HONEST FIX: Uses external sidechain bus or internal LFO (Sync'd to BPM).
 * Provides 'The Bounce' found in modern Logic Pro productions.
 */
class SidechainDuck : public IProcessor {
public:
    SidechainDuck() = default;

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        m_sampleRate = std::isfinite(sr) && sr > 1000.0 ? sr : 44100.0;
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        const uint32_t n = buffer.getNumSamples();
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!left || n == 0) return;
        const Core::AudioBuffer* sidechain = context.sidechainBuffer;
        const double bpm = std::isfinite(context.bpm) && context.bpm > 1.0 ? context.bpm : 120.0;
        const double lfoHz = bpm / 60.0;
        const float depth = std::clamp(std::isfinite(m_depth) ? m_depth : 0.0f, 0.0f, 1.0f);
        const float attack = std::exp(-1.0f / (0.005f * static_cast<float>(m_sampleRate)));
        const float release = std::exp(-1.0f / (0.080f * static_cast<float>(m_sampleRate)));
        for (uint32_t i = 0; i < n; ++i) {
            float detector = 0.0f;
            if (sidechain && sidechain->getNumChannels() > 0 && i < sidechain->getNumSamples()) {
                detector = std::abs(sidechain->getReadPointer(0)[i]);
                if (sidechain->getNumChannels() > 1) detector = std::max(detector, std::abs(sidechain->getReadPointer(1)[i]));
            } else {
                detector = 0.5f + 0.5f * std::sin(2.0 * M_PI * m_lfoPhase);
                m_lfoPhase += lfoHz / m_sampleRate;
                if (m_lfoPhase >= 1.0) m_lfoPhase -= std::floor(m_lfoPhase);
            }
            const float target = 1.0f - depth * std::clamp(detector, 0.0f, 1.0f);
            m_currentGain = target < m_currentGain ? attack * m_currentGain + (1.0f - attack) * target
                                                     : release * m_currentGain + (1.0f - release) * target;
            left[i] *= m_currentGain;
            if (right) right[i] *= m_currentGain;
        }
    }


    void reset() noexcept override {
        m_currentGain = 1.0f;
        m_lfoPhase = 0.0;
    }

    void setDepth(float d) { m_depth = std::clamp(d, 0.0f, 1.0f); }

private:
    double m_sampleRate = 44100.0;
    float m_depth = 0.8f;
    float m_currentGain = 1.0f;
    double m_lfoPhase = 0.0;
};

} // namespace Aura::DSP::Effects
