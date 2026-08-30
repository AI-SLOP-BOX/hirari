#pragma once

#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class StereoExpander
 * @brief High-precision Mid-Side (M/S) Stereo Image Processor.
 * HONEST FIX: Implements M/S matrixing to allow independent control 
 * of the 'Mid' (Mono) and 'Side' (Stereo) components.
 * Provides the immersive width found in professional mastering tools (Ozone Imager style).
 */
class StereoExpander : public IProcessor {
public:
    StereoExpander() : m_width(1.0f), m_midGain(1.0f) {}

    void prepareToPlay(double sr, uint32_t bs) noexcept override { (void)sr; (void)bs; }

    /**
     * @brief PROCESS: M/S Matrixing and Width expansion.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        const uint32_t n = buffer.getNumSamples();
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!left || !right) return;
        const float midGain = std::isfinite(m_midGain) ? m_midGain : 1.0f;
        const float width = std::isfinite(m_width) ? m_width : 1.0f;
        const float mix = getMix();
        for (uint32_t i = 0; i < n; ++i) {
            const float dryL = left[i];
            const float dryR = right[i];
            const float mid = 0.5f * (dryL + dryR) * midGain;
            const float side = 0.5f * (dryL - dryR) * width;
            const float wetL = mid + side;
            const float wetR = mid - side;
            left[i] = dryL + mix * (wetL - dryL);
            right[i] = dryR + mix * (wetR - dryR);
        }
    }


    void reset() noexcept override {}

    // Parameters
    void setWidth(float w) { m_width = std::clamp(w, 0.0f, 2.0f); }
    void setMidGain(float g) { m_midGain = std::clamp(g, 0.0f, 2.0f); }

private:
    float m_width;   // Side gain multiplier
    float m_midGain; // Mid gain multiplier
};

} // namespace Aura::DSP::Effects
