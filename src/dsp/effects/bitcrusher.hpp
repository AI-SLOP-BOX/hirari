#pragma once

#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class Bitcrusher
 * @brief Professional Digital Lo-fi and Character Distortion.
 * HONEST FIX: Implements combined Bit-depth reduction (Quantization) 
 * and Sample-rate reduction (Sample-and-hold) for intentional alias grittiness.
 * Essential for modern Pop and Electronic production artifacts.
 */
class Bitcrusher : public IProcessor {
public:
    Bitcrusher() : m_bits(16.0f), m_downsample(1.0f), m_holdSampleL(0.0f), m_holdSampleR(0.0f), m_sampleCounter(0.0f) {}

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        reset();
    }

    /**
     * @brief PROCESS: Quantizes and downsamples the signal.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumSamples() == 0) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        const float levels = std::ldexp(1.0f, static_cast<int>(std::clamp(m_bits, 1.0f, 24.0f)) - 1);
        const uint32_t hold = std::max<uint32_t>(1, static_cast<uint32_t>(std::ceil(m_downsample)));
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            if (m_sampleCounter <= 0.0f) {
                m_holdSampleL = std::round(buffer.getReadPointer(0)[i] * levels) / levels;
                m_holdSampleR = channels > 1 ? std::round(buffer.getReadPointer(1)[i] * levels) / levels : m_holdSampleL;
                m_sampleCounter = static_cast<float>(hold);
            }
            --m_sampleCounter;
            const float dryL = buffer.getReadPointer(0)[i];
            const float dryR = channels > 1 ? buffer.getReadPointer(1)[i] : dryL;
            buffer.getWritePointer(0)[i] = dryL * (1.0f - m_mix) + m_holdSampleL * m_mix;
            if (channels > 1) buffer.getWritePointer(1)[i] = dryR * (1.0f - m_mix) + m_holdSampleR * m_mix;
        }
    }


    void reset() noexcept override {
        m_holdSampleL = 0.0f;
        m_holdSampleR = 0.0f;
        m_sampleCounter = 0.0f;
    }

    // Parameters
    void setBits(float b) { m_bits = std::clamp(b, 1.0f, 24.0f); }
    void setDownsample(float d) { m_downsample = std::max(1.0f, d); }

private:
    float m_bits;
    float m_downsample;
    float m_holdSampleL, m_holdSampleR;
    float m_sampleCounter;
};

} // namespace Aura::DSP::Effects
