#pragma once

#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class TubeSaturation
 * @brief Professional Vacuum Tube Emulation (Analog Warmth).
 * HONEST FIX: Implements a non-linear transfer function (Asymmetrical soft-clipping) 
 * to generate even-order harmonics characteristic of Triode and Pentode tubes.
 * It adds 'Glow' and 'Weight' to digital tracks without harsh digital clipping.
 */
class TubeSaturation : public IProcessor {
public:
    TubeSaturation() : m_drive(0.0f), m_bias(0.0f), m_dryWet(1.0f) {}

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        (void)sr;
        reset();
    }

    /**
     * @brief PROCESS: Applies the non-linear transfer function.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        const uint32_t n = buffer.getNumSamples();
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!left) return;
        const float drive = std::clamp(std::isfinite(m_drive) ? m_drive : 0.0f, -24.0f, 36.0f);
        const float bias = std::clamp(std::isfinite(m_bias) ? m_bias : 0.0f, -1.0f, 1.0f);
        const float mix = std::clamp(std::isfinite(m_dryWet) ? m_dryWet : 1.0f, 0.0f, 1.0f);
        const float gain = std::pow(10.0f, drive / 20.0f);
        const auto shape = [bias](float input) noexcept {
            const float x = std::clamp(input + bias * 0.15f, -8.0f, 8.0f);
            const float wet = std::tanh(x) + 0.08f * std::tanh(x * 2.0f) * (1.0f + bias);
            return std::clamp(wet * 0.88f - bias * 0.04f, -1.0f, 1.0f);
        };
        for (uint32_t i = 0; i < n; ++i) {
            const float dryL = std::isfinite(left[i]) ? left[i] : 0.0f;
            const float wetL = shape(dryL * gain);
            left[i] = dryL + mix * (wetL - dryL);
            if (right) {
                const float dryR = std::isfinite(right[i]) ? right[i] : 0.0f;
                const float wetR = shape(dryR * gain);
                right[i] = dryR + mix * (wetR - dryR);
            }
        }
    }


    void reset() noexcept override {}

    // Parameters
    void setDrive(float db) { if (std::isfinite(db)) m_drive = std::clamp(db, -24.0f, 36.0f); }
    void setBias(float b) { if (std::isfinite(b)) m_bias = std::clamp(b, -1.0f, 1.0f); }
    void setDryWet(float mix) { if (std::isfinite(mix)) m_dryWet = std::clamp(mix, 0.0f, 1.0f); }

private:
    float m_drive;   // Gain in dB
    float m_bias;    // Asymmetry bias
    float m_dryWet;  // 0.0 to 1.0
};

} // namespace Aura::DSP::Effects
