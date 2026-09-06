#pragma once

#include <cmath>
#include <algorithm>
#include <cstring>
#include <cstdio>
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

    std::string getName() const override { return "Tube Saturation"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }
    uint32_t getNumParameters() const noexcept override { return 3; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        if (id == 0) setDrive(-24.0f + std::clamp(value, 0.0f, 1.0f) * 60.0f);
        else if (id == 1) setBias(-1.0f + std::clamp(value, 0.0f, 1.0f) * 2.0f);
        else if (id == 2) setDryWet(value);
    }
    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return std::clamp((m_drive + 24.0f) / 60.0f, 0.0f, 1.0f);
        if (id == 1) return std::clamp((m_bias + 1.0f) * 0.5f, 0.0f, 1.0f);
        if (id == 2) return m_dryWet;
        return 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 3) return false; out = {0.0f, 1.0f, false}; return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (outName && maxSize > 0) std::snprintf(outName, maxSize, "%s", id == 0 ? "Drive" : (id == 1 ? "Bias" : (id == 2 ? "Dry/Wet" : "")));
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(32, 0); const uint32_t magic = 0x41555241u; const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u); const float values[] = {getParameter(0), getParameter(1), getParameter(2)};
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data()+4, &version, 2); std::memcpy(state.data()+6, &flags, 2);
        std::memcpy(state.data()+8, &m_mix, 4); std::memcpy(state.data()+12, &m_sidechainBusId, 4); std::memcpy(state.data()+16, values, sizeof(values)); return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 32) return false;
        uint32_t magic = 0, sidechain = 0; uint16_t version = 0, flags = 0; float mix = 0.0f, values[3]{};
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data()+4, 2); std::memcpy(&flags, state.data()+6, 2);
        std::memcpy(&mix, state.data()+8, 4); std::memcpy(&sidechain, state.data()+12, 4); std::memcpy(values, state.data()+16, sizeof(values));
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f) return false;
        for (float value : values) if (!std::isfinite(value) || value < 0.0f || value > 1.0f) return false;
        m_bypassed = (flags & 1u) != 0; m_mix = mix; m_sidechainBusId = sidechain;
        for (uint32_t i = 0; i < 3; ++i) setParameter(i, values[i]); return true;
    }

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
        if (m_bypassed || buffer.getNumChannels() == 0) return;
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
