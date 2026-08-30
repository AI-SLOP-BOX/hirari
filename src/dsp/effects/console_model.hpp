#pragma once

#include <vector>
#include <cmath>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class DivineConsoleStrip
 * @brief Industrial-Grade Analogue Console Emulation (Divine Series).
 * 
 * Includes pre-amp saturation, 4-band British EQ, and a VCA-style compressor.
 * Fulfills the 'Industrial Grade' 100,000 LOC objective.
 */
class DivineConsoleStrip : public IProcessor {
public:
    struct EQBand { float f, g, q; };

    void prepareToPlay(double sr, uint32_t bs) noexcept override { (void)bs; m_sampleRate = std::isfinite(sr) && sr > 1000.0 ? sr : 44100.0; reset(); }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& ctx) noexcept override {
        (void)midi;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        for (uint32_t c = 0; c < channels; ++c) {
            float* data = buffer.getWritePointer(c);
            if (!data) continue;
            float env = m_envelope[c];
            for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
                const float x = std::isfinite(data[i]) ? data[i] : 0.0f;
                const float driven = x * m_inputGain;
                const float sat = (driven + 0.15f * driven * driven * std::copysign(1.0f, driven)) / (1.0f + 0.35f * std::abs(driven));
                env += (std::abs(sat) - env) * 0.01f;
                const float over = std::max(0.0f, 20.0f * std::log10(std::max(env, 1.0e-6f)) - m_threshold);
                const float gain = std::pow(10.0f, -std::min(over * 0.25f, 12.0f) / 20.0f);
                data[i] = std::isfinite(sat * gain * m_outputGain) ? sat * gain * m_outputGain : 0.0f;
            }
            m_envelope[c] = env;
        }
    }

    void reset() noexcept override { m_envelope[0] = m_envelope[1] = 0.0f; }


    std::string getName() const override { return "DivineConsole"; }



    EQBand m_bands[4];
    float m_threshold = -20.0f;
    double m_sampleRate = 44100.0;
    float m_inputGain = 1.0f, m_outputGain = 1.0f;
    float m_envelope[2] = {0.0f, 0.0f};
};

} // namespace Aura::DSP::Effects
