#pragma once
#include <vector>
#include <cmath>
#include <algorithm>
#include <cstring>
#include <cstdio>
#include <atomic>
#include "../iprocessor.hpp"
#include "../utils/dsp_utils.hpp"

namespace Aura::DSP::Effects {

/**
 * @class AutoFilter
 * @brief Professional Dynamic Resonant Filter (Auto-Wah).
 */
class AutoFilter : public IProcessor {
public:
    AutoFilter() : m_cutoffBase(0.2f), m_res(0.3f), m_sens(0.8f), m_env(0.0f) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = (std::isfinite(sr) && sr >= 8000.0 && sr <= 384000.0) ? sr : 44100.0;
        updateTimeConstants();
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            float input[2] = {buffer.getReadPointer(0)[i], channels > 1 ? buffer.getReadPointer(1)[i] : buffer.getReadPointer(0)[i]};
            const float detector = 0.5f * (std::abs(input[0]) + std::abs(input[1]));
            const float coeff = detector > m_env ? m_attack : m_release;
            m_env += (detector - m_env) * coeff;
            const float cutoffBase = m_cutoffBase.load(std::memory_order_relaxed);
            const float resonance = m_res.load(std::memory_order_relaxed);
            const float sensitivity = m_sens.load(std::memory_order_relaxed);
            const float cutoffNorm = std::clamp(cutoffBase + m_env * sensitivity * 0.7f, 0.01f, 0.49f);
            const float f = 2.0f * std::sin(3.14159265f * cutoffNorm);
            const float damp = std::clamp(2.0f * (1.0f - std::pow(resonance, 0.25f)), 0.05f, 2.0f);
            for (uint32_t c = 0; c < channels; ++c) {
                const float low = m_s1[c] + f * m_s2[c];
                const float high = input[c] - low - damp * m_s2[c];
                const float band = f * high + m_s2[c];
                m_s1[c] = low;
                m_s2[c] = band;
                const float wet = low + band * 0.35f;
                buffer.getWritePointer(c)[i] = input[c] * (1.0f - m_mix) + wet * m_mix;
            }
        }
    }


    void reset() noexcept override {
        m_env = 0.0f;
        m_s1[0] = m_s1[1] = 0.0f;
        m_s2[0] = m_s2[1] = 0.0f;
    }

    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        if (id == 0) m_cutoffBase.store(std::clamp(value, 0.0f, 1.0f), std::memory_order_relaxed);
        else if (id == 1) m_res.store(std::clamp(value, 0.0f, 1.0f), std::memory_order_relaxed);
        else if (id == 2) m_sens.store(std::clamp(value, 0.0f, 1.0f), std::memory_order_relaxed);
        else if (id == 3) m_mix = std::clamp(value, 0.0f, 1.0f);
    }

    std::string getName() const override { return "Auto Filter"; }
    uint32_t getNumParameters() const noexcept override { return 4; }
    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return m_cutoffBase.load(std::memory_order_relaxed);
        if (id == 1) return m_res.load(std::memory_order_relaxed);
        if (id == 2) return m_sens.load(std::memory_order_relaxed);
        if (id == 3) return m_mix;
        return 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 4) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        const char* names[] = {"Cutoff", "Resonance", "Sensitivity", "Mix"};
        std::snprintf(outName, maxSize, "%s", id < 4 ? names[id] : "");
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(32, 0);
        const uint32_t magic = 0x41555241u; const uint16_t version = 1;
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data()+4, &version, 2);
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        std::memcpy(state.data()+6, &flags, 2); std::memcpy(state.data()+8, &m_sidechainBusId, 4);
        const float values[] = {m_cutoffBase.load(std::memory_order_relaxed),
                                m_res.load(std::memory_order_relaxed),
                                m_sens.load(std::memory_order_relaxed), m_mix};
        std::memcpy(state.data()+16, values, sizeof(values));
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 32) return false;
        uint32_t magic = 0; uint16_t version = 0, flags = 0; uint32_t sidechain = 0;
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data()+4, 2);
        std::memcpy(&flags, state.data()+6, 2); std::memcpy(&sidechain, state.data()+8, 4);
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0) return false;
        float values[4]{}; std::memcpy(values, state.data()+16, sizeof(values));
        for (float value : values) if (!std::isfinite(value) || value < 0.0f || value > 1.0f) return false;
        for (uint32_t i = 0; i < 4; ++i) setParameter(i, values[i]);
        m_bypassed = (flags & 1u) != 0; m_sidechainBusId = sidechain;
        return true;
    }

private:
    void updateTimeConstants() {
        m_attack = 1.0f - std::exp(-1.0f / (0.005f * m_sampleRate)); // 5ms
        m_release = 1.0f - std::exp(-1.0f / (0.100f * m_sampleRate)); // 100ms
    }

    double m_sampleRate = 44100.0;
    std::atomic<float> m_cutoffBase, m_res, m_sens;
    float m_env, m_attack, m_release;
    float m_g = 0, m_k = 0;
    float m_s1[2] = {0,0}, m_s2[2] = {0,0}; // Filter states
};

} // namespace Aura::DSP::Effects
