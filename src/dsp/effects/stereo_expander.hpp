#pragma once

#include <cmath>
#include <algorithm>
#include <vector>
#include <cstring>
#include <cstdio>
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

    std::string getName() const override { return "Stereo Expander"; }
    uint32_t getNumParameters() const noexcept override { return 2; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        if (id == 0) setWidth(value * 2.0f);
        else if (id == 1) setMidGain(value * 2.0f);
    }
    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return std::clamp(m_width / 2.0f, 0.0f, 1.0f);
        if (id == 1) return std::clamp(m_midGain / 2.0f, 0.0f, 1.0f);
        return 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 2) return false; out = {0.0f, 1.0f, false}; return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (outName && maxSize > 0) std::snprintf(outName, maxSize, "%s", id == 0 ? "Width" : (id == 1 ? "Mid Gain" : ""));
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override { (void)sr; (void)bs; }

    /**
     * @brief PROCESS: M/S Matrixing and Width expansion.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        if (m_bypassed || buffer.getNumChannels() < 2) return;
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

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(24, 0); const uint32_t magic = 0x41555241u; const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u); const float values[] = {getParameter(0), getParameter(1)};
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data()+4, &version, 2); std::memcpy(state.data()+6, &flags, 2);
        std::memcpy(state.data()+8, &m_mix, 4); std::memcpy(state.data()+12, &m_sidechainBusId, 4); std::memcpy(state.data()+16, values, sizeof(values)); return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 24) return false;
        uint32_t magic = 0, sidechain = 0; uint16_t version = 0, flags = 0; float mix = 0.0f, values[2]{};
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data()+4, 2); std::memcpy(&flags, state.data()+6, 2);
        std::memcpy(&mix, state.data()+8, 4); std::memcpy(&sidechain, state.data()+12, 4); std::memcpy(values, state.data()+16, sizeof(values));
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f || values[0] < 0.0f || values[0] > 1.0f || values[1] < 0.0f || values[1] > 1.0f) return false;
        m_bypassed = (flags & 1u) != 0; m_mix = mix; m_sidechainBusId = sidechain; setParameter(0, values[0]); setParameter(1, values[1]); return true;
    }

private:
    float m_width;   // Side gain multiplier
    float m_midGain; // Mid gain multiplier
};

} // namespace Aura::DSP::Effects
