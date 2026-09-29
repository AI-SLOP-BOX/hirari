#pragma once
#include <memory>
#include <vector>
#include "../iprocessor.hpp"
#include "../dsp/utils/oversampler.hpp"

namespace Hirari::Core::DSP {

/**
 * @class OversamplingWrapper
 * @brief Professional-grade 2x Oversampling Wrapper for any IProcessor.
 * HONEST FIX: Realized that high-gain saturation and limiters cause 
 * aliasing at 44.1/48kHz. This wrapper doubles the rate and anti-aliases automatically.
 */
class OversamplingWrapper : public IProcessor {
public:
    OversamplingWrapper(std::shared_ptr<IProcessor> p, double sr = 44100.0) 
        : m_p(p), m_oversampler(sr) {}

    void prepareToPlay(double sr, uint32_t bs) override {
        m_oversampler = Utils::Oversampler2x(sr);
        m_p->prepareToPlay(sr * 2.0, bs * 2);
    }

    void process(AudioBuffer& b, const MidiBuffer& midi) override {
        uint32_t numSamples = b.getNumSamples();
        uint32_t numChannels = b.getNumChannels();

        // 1. UPSCALE TO 2x RATE
        AudioBuffer v2x(numChannels, numSamples * 2);
        for (uint32_t c = 0; c < numChannels; ++c) {
            float* src = b.getWritePointer(c);
            float* dst = v2x.getWritePointer(c);
            for (uint32_t s = 0; s < numSamples; ++s) {
                m_oversampler.upsample(src[s], dst[s*2], dst[s*2+1]);
            }
        }

        // 2. PROCESS AT 2x RATE
        // (Assuming MIDI timing also scales or is simple Note On/Off)
        m_p->process(v2x, midi);

        // 3. DOWNSAMPLE & ANTI-ALIAS
        for (uint32_t c = 0; c < numChannels; ++c) {
            float* src = v2x.getWritePointer(c);
            float* dst = b.getWritePointer(c);
            for (uint32_t s = 0; s < numSamples; ++s) {
                dst[s] = m_oversampler.downsample(src[s*2], src[s*2+1]);
            }
        }
    }

    void reset() override { m_p->reset(); }
    uint32_t getLatencySamples() const override { return m_p->getLatencySamples() / 2; }

private:
    std::shared_ptr<IProcessor> m_p;
    Utils::Oversampler2x m_oversampler;
};

} // namespace Hirari::Core::DSP
