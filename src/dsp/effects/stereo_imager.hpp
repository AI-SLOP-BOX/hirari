#pragma once

#include <cmath>
#include <algorithm>
#include <cstdio>
#include "../iprocessor.hpp"
#include "../math/fast_math.hpp"

namespace Aura::DSP::Effects {

/**
 * @brief StereoImager: Advanced Mid-Side spatial processor.
 * Controls the width and spatial distribution of the stereo field.
 */
class StereoImager : public IProcessor {
public:
    StereoImager(double sr = 44100.0) : m_sampleRate(
        std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44'100.0) {
        setWidth(1.0f); // Neutral width
    }

    std::string getName() const override { return "Stereo Imager"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0 && std::isfinite(value)) setWidth(value * 4.0f); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? std::clamp(m_width / 4.0f, 0.0f, 1.0f) : 0.0f; }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id != 0) return false; out = {0.0f, 1.0f, false}; return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (outName && maxSize > 0) std::snprintf(outName, maxSize, "%s", id == 0 ? "Width" : "");
    }

    /**
     * @brief Sets the stereo width factor.
     * @param width: 0.0 (Mono), 1.0 (Neutral), > 1.0 (Widened).
     */
    void setWidth(float width) {
        m_width = std::clamp(width, 0.0f, 4.0f);
        m_targetSideGain = m_width;
    }

    void prepareToPlay(double sr, uint32_t) noexcept override {
        setSampleRate(sr);
        reset();
    }

    void reset() noexcept override {
        m_currentSideGain = std::clamp(m_targetSideGain, 0.0f, 4.0f);
    }

    void process(float* l, float* r, uint32_t numSamples) {
        if (!l || !r || numSamples == 0 || m_bypassed) return;
        const float target = std::clamp(m_targetSideGain, 0.0f, 4.0f);
        for (uint32_t i = 0; i < numSamples; ++i) {
            m_currentSideGain += (target - m_currentSideGain) * 0.01f;
            const float mid = 0.5f * (l[i] + r[i]);
            const float side = 0.5f * (l[i] - r[i]) * m_currentSideGain;
            const float wetL = mid + side;
            const float wetR = mid - side;
            l[i] = l[i] * (1.0f - m_mix) + wetL * m_mix;
            r[i] = r[i] * (1.0f - m_mix) + wetR * m_mix;
        }
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const ProcessContext&) noexcept override {
        if (buffer.getNumChannels() < 2) return;
        process(buffer.getWritePointer(0), buffer.getWritePointer(1), buffer.getNumSamples());
    }


    void setSampleRate(double sr) {
        if (std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0) m_sampleRate = sr;
    }
    uint32_t getLatencySamples() const noexcept override { return 0; }

private:
    double m_sampleRate;
    float m_width = 1.0f;
    float m_targetSideGain = 1.0f, m_currentSideGain = 1.0f;
    Math::FastMath m_math;
};

} // namespace Aura::DSP::Effects
