#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"
#include "../effects/delay_line.hpp"

namespace Aura::DSP::Effects {

/**
 * @brief VocalDoubler: Industrial-standard vocal thickening.
 * Creates 'Double' takes automatically with micro-timing and pitch shifts.
 */
class VocalDoubler : public IProcessor {
public:
    VocalDoubler(double sr = 44100.0) : m_sampleRate(std::isfinite(sr) && sr > 1000.0 ? sr : 44100.0) {}

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        m_sampleRate = std::isfinite(sr) && sr > 1000.0 ? sr : 44100.0;
        m_delayL = DelayLine(static_cast<uint32_t>(m_sampleRate * 0.1) + 2u);
        m_delayR = DelayLine(static_cast<uint32_t>(m_sampleRate * 0.1) + 2u);
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        process(left, right, buffer.getNumSamples());
    }

    void process(float* l, float* r, uint32_t numSamples) noexcept {
        if (!l || !r || numSamples == 0) return;
        const float mix = std::clamp(std::isfinite(m_mix) ? m_mix : 0.0f, 0.0f, 1.0f);
        const float depth = std::clamp(std::isfinite(m_depth) ? m_depth : 1.0f, 0.0f, 1.0f);
        const double sr = m_sampleRate > 1000.0 ? m_sampleRate : 44100.0;
        for (uint32_t i = 0; i < numSamples; ++i) {
            const float phase = static_cast<float>(2.0 * M_PI * m_phase);
            const uint32_t delayL = static_cast<uint32_t>(std::clamp(
                (0.018f + 0.006f * std::sin(phase)) * sr, 1.0, sr * 0.1));
            const uint32_t delayR = static_cast<uint32_t>(std::clamp(
                (0.024f + 0.006f * std::sin(phase + static_cast<float>(M_PI))) * sr, 1.0, sr * 0.1));
            const float dryL = std::isfinite(l[i]) ? l[i] : 0.0f;
            const float dryR = std::isfinite(r[i]) ? r[i] : 0.0f;
            const float doubledL = m_delayL.process(dryL, delayL);
            const float doubledR = m_delayR.process(dryR, delayR);
            l[i] = dryL + mix * depth * 0.55f * (doubledL - dryL);
            r[i] = dryR + mix * depth * 0.55f * (doubledR - dryR);
            m_phase += 0.2 / sr;
            if (m_phase >= 1.0) m_phase -= std::floor(m_phase);
        }
    }

    void reset() noexcept override {
        m_delayL.reset();
        m_delayR.reset();
        m_phase = 0.0;
    }

    void setSampleRate(double sr) { if (std::isfinite(sr) && sr > 1000.0) m_sampleRate = sr; }
    void setMix(float mix) noexcept { if (std::isfinite(mix)) m_mix = std::clamp(mix, 0.0f, 1.0f); }
    void setDepth(float depth) noexcept { if (std::isfinite(depth)) m_depth = std::clamp(depth, 0.0f, 1.0f); }
    uint32_t getLatencySamples() const noexcept override { return 0; }

private:
    double m_sampleRate;
    double m_phase = 0.0;
    float m_mix = 0.5f;
    float m_depth = 1.0f;
    DelayLine m_delayL{44102};
    DelayLine m_delayR{44102};
};

} // namespace Aura::DSP::Effects
