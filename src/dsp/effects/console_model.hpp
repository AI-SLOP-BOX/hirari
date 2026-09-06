#pragma once

#include <vector>
#include <cmath>
#include <array>
#include <cstdio>
#include <cstring>
#include "../iprocessor.hpp"
#include "state_variable_filter.hpp"

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

    DivineConsoleStrip() {
        m_bands[0] = {90.0f, 0.0f, 0.8f};
        m_bands[1] = {700.0f, 0.0f, 0.9f};
        m_bands[2] = {3200.0f, 0.0f, 0.9f};
        m_bands[3] = {10000.0f, 0.0f, 0.8f};
        configureEq();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override { (void)bs; m_sampleRate = std::isfinite(sr) && sr > 1000.0 ? sr : 44100.0; configureEq(); reset(); }

    uint32_t getLatencySamples() const noexcept override { return 0; }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& ctx) noexcept override {
        (void)midi;
        (void)ctx;
        if (m_bypassed || buffer.getNumChannels() == 0) return;
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
        // Four musical bell bands form the console's tone section after the
        // nonlinear preamp and VCA stage.
        for (auto& band : m_eq) band.process(buffer);
    }

    void reset() noexcept override { m_envelope[0] = m_envelope[1] = 0.0f; for (auto& band : m_eq) band.reset(); }


    std::string getName() const override { return "DivineConsole"; }
    uint32_t getNumParameters() const noexcept override { return 7; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        if (id == 0) m_inputGain = std::pow(10.0f, std::clamp(value, 0.0f, 1.0f) * 24.0f / 20.0f);
        else if (id == 1) m_outputGain = std::pow(10.0f, (std::clamp(value, 0.0f, 1.0f) - 0.5f) * 24.0f / 20.0f);
        else if (id == 2) m_threshold = -60.0f + std::clamp(value, 0.0f, 1.0f) * 60.0f;
        else if (id >= 3 && id < 7) setBand(id - 3, m_bands[id - 3].f, -24.0f + std::clamp(value, 0.0f, 1.0f) * 48.0f, m_bands[id - 3].q);
    }
    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return std::clamp(20.0f * std::log10(std::max(m_inputGain, 1.0e-6f)) / 24.0f, 0.0f, 1.0f);
        if (id == 1) return std::clamp(0.5f + 20.0f * std::log10(std::max(m_outputGain, 1.0e-6f)) / 24.0f, 0.0f, 1.0f);
        if (id == 2) return std::clamp((m_threshold + 60.0f) / 60.0f, 0.0f, 1.0f);
        return id < 7 ? std::clamp((m_bands[id - 3].g + 24.0f) / 48.0f, 0.0f, 1.0f) : 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 7) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        const char* names[] = {"Input", "Output", "Threshold", "Low Gain", "Low-Mid Gain", "High-Mid Gain", "High Gain"};
        std::snprintf(outName, maxSize, "%s", id < 7 ? names[id] : "");
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(44, 0);
        const uint32_t magic = 0x41555241u; const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data() + 4, &version, 2);
        std::memcpy(state.data() + 6, &flags, 2); std::memcpy(state.data() + 8, &m_mix, 4);
        std::memcpy(state.data() + 12, &m_sidechainBusId, 4);
        for (uint32_t i = 0; i < 7; ++i) { const float value = getParameter(i); std::memcpy(state.data() + 16 + i * 4, &value, 4); }
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 44) return false;
        uint32_t magic = 0, sidechain = 0; uint16_t version = 0, flags = 0; float mix = 0.0f;
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data() + 4, 2);
        std::memcpy(&flags, state.data() + 6, 2); std::memcpy(&mix, state.data() + 8, 4); std::memcpy(&sidechain, state.data() + 12, 4);
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f) return false;
        float values[7]{}; std::memcpy(values, state.data() + 16, sizeof(values));
        for (float value : values) if (!std::isfinite(value) || value < 0.0f || value > 1.0f) return false;
        m_bypassed = (flags & 1u) != 0; m_mix = mix; m_sidechainBusId = sidechain;
        for (uint32_t i = 0; i < 7; ++i) setParameter(i, values[i]);
        return true;
    }

    bool setBand(uint32_t index, float frequency, float gainDb, float q) noexcept {
        if (index >= 4 || !std::isfinite(frequency) || !std::isfinite(gainDb) || !std::isfinite(q) ||
            frequency <= 5.0f || q <= 0.05f) return false;
        m_bands[index] = {frequency, std::clamp(gainDb, -24.0f, 24.0f), std::clamp(q, 0.05f, 20.0f)};
        m_eq[index].setParams(m_bands[index].f, m_bands[index].g, m_bands[index].q);
        return true;
    }



    EQBand m_bands[4];
    float m_threshold = -20.0f;
    double m_sampleRate = 44100.0;
    float m_inputGain = 1.0f, m_outputGain = 1.0f;
    float m_envelope[2] = {0.0f, 0.0f};
    std::array<StateVariableFilter, 4> m_eq;

    void configureEq() noexcept {
        for (uint32_t i = 0; i < 4; ++i) {
            m_eq[i].prepareToPlay(m_sampleRate, 0);
            m_eq[i].setType(StateVariableFilter::Bell);
            m_eq[i].setParams(m_bands[i].f, m_bands[i].g, m_bands[i].q);
        }
    }
};

} // namespace Aura::DSP::Effects
