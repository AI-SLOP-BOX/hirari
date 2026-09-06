#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include <cstdio>
#include <cstring>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class StereoPhaser
 * @brief High-end Modulation effect with multi-stage phase shifting.
 * HONEST FIX: Implements 4-stage All-Pass filters with a stereo-offset LFO 
 * to create the iconic 'sweeping' movement.
 * Provides the psychedelic depth found in high-end modulation pedals.
 */
class StereoPhaser : public IProcessor {
public:
    StereoPhaser() {
        reset();
    }

    std::string getName() const override { return "Stereo Phaser"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }
    uint32_t getNumParameters() const noexcept override { return 3; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        if (id == 0) setRate(0.01f + value * 19.99f);
        else if (id == 1) setFeedback(-0.95f + value * 1.9f);
        else if (id == 2) setMix(value);
    }
    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return std::clamp((m_rate - 0.01f) / 19.99f, 0.0f, 1.0f);
        if (id == 1) return std::clamp((m_feedback + 0.95f) / 1.9f, 0.0f, 1.0f);
        return id == 2 ? m_mix : 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 3) return false; out = {0.0f, 1.0f, false}; return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        const char* names[] = {"Rate", "Feedback", "Mix"};
        std::snprintf(outName, maxSize, "%s", id < 3 ? names[id] : "");
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(28, 0);
        const uint32_t magic = 0x41555241u; const uint16_t version = 1; const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data() + 4, &version, 2); std::memcpy(state.data() + 6, &flags, 2);
        std::memcpy(state.data() + 8, &m_mix, 4); std::memcpy(state.data() + 12, &m_sidechainBusId, 4);
        const float values[3] = {getParameter(0), getParameter(1), getParameter(2)}; std::memcpy(state.data() + 16, values, sizeof(values));
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 28) return false;
        uint32_t magic = 0, sidechain = 0; uint16_t version = 0, flags = 0; float mix = 0.0f, values[3]{};
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data() + 4, 2); std::memcpy(&flags, state.data() + 6, 2);
        std::memcpy(&mix, state.data() + 8, 4); std::memcpy(&sidechain, state.data() + 12, 4); std::memcpy(values, state.data() + 16, sizeof(values));
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f) return false;
        for (float value : values) if (!std::isfinite(value) || value < 0.0f || value > 1.0f) return false;
        m_bypassed = (flags & 1u) != 0; m_mix = mix; m_sidechainBusId = sidechain;
        for (uint32_t i = 0; i < 3; ++i) setParameter(i, values[i]);
        return true;
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        m_sampleRate = std::isfinite(sr) && sr > 1000.0 ? sr : 44100.0;
        reset();
    }

    /**
     * @brief PROCESS: Modulates phase cancellaton points over time.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        const uint32_t n = buffer.getNumSamples();
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!left || n == 0) return;

        const float mix = std::clamp(std::isfinite(m_mix) ? m_mix : 0.0f, 0.0f, 1.0f);
        const float feedback = std::clamp(std::isfinite(m_feedback) ? m_feedback : 0.0f, -0.95f, 0.95f);
        const float rate = std::clamp(std::isfinite(m_rate) ? m_rate : 0.5f, 0.01f, 20.0f);
        const float phaseInc = rate / static_cast<float>(m_sampleRate);

        for (uint32_t i = 0; i < n; ++i) {
            const float dryL = std::isfinite(left[i]) ? left[i] : 0.0f;
            const float dryR = right && std::isfinite(right[i]) ? right[i] : dryL;
            const float lfoL = 0.5f + 0.5f * std::sin(2.0f * static_cast<float>(M_PI) * m_lfoPhase);
            const float lfoR = 0.5f + 0.5f * std::sin(2.0f * static_cast<float>(M_PI) * (m_lfoPhase + 0.5f));
            const float coeffL = 0.05f + 0.90f * lfoL;
            const float coeffR = 0.05f + 0.90f * lfoR;
            float wetL = dryL + m_lastOut[0] * feedback;
            float wetR = dryR + m_lastOut[1] * feedback;
            for (size_t stage = 0; stage < 4; ++stage) {
                const float aL = std::clamp(coeffL * (0.85f + 0.04f * static_cast<float>(stage)), -0.98f, 0.98f);
                const float aR = std::clamp(coeffR * (0.85f + 0.04f * static_cast<float>(stage)), -0.98f, 0.98f);
                float outL = -aL * wetL + m_filterState[0][stage];
                float outR = -aR * wetR + m_filterState[1][stage];
                m_filterState[0][stage] = wetL + aL * outL;
                m_filterState[1][stage] = wetR + aR * outR;
                wetL = outL;
                wetR = outR;
            }
            m_lastOut[0] = std::isfinite(wetL) ? wetL : 0.0f;
            m_lastOut[1] = std::isfinite(wetR) ? wetR : 0.0f;
            left[i] = dryL + mix * (m_lastOut[0] - dryL);
            if (right) right[i] = dryR + mix * (m_lastOut[1] - dryR);
            m_lfoPhase += phaseInc;
            if (m_lfoPhase >= 1.0f) m_lfoPhase -= std::floor(m_lfoPhase);
        }
    }


    void reset() noexcept override {
        for (auto& v : m_filterState) std::fill(v.begin(), v.end(), 0.0f);
        m_lastOut[0] = m_lastOut[1] = 0.0f;
    }

    uint32_t getTailSamples() const noexcept override {
        const double rate = std::isfinite(m_sampleRate) && m_sampleRate > 0.0
            ? m_sampleRate : 44'100.0;
        const double feedback = std::clamp(std::abs(m_feedback), 0.0f, 0.95f);
        const double tailSeconds = feedback > 0.0
            ? std::min(2.0, 0.35 * std::log(1.0e-3) / std::log(feedback))
            : 0.0;
        return static_cast<uint32_t>(tailSeconds * rate);
    }

    // Parameters
    void setMix(float m) { if (std::isfinite(m)) m_mix = std::clamp(m, 0.0f, 1.0f); }
    void setRate(float r) { if (std::isfinite(r)) m_rate = std::clamp(r, 0.01f, 20.0f); }
    void setFeedback(float f) { if (std::isfinite(f)) m_feedback = std::clamp(f, -0.95f, 0.95f); }

private:
    double m_sampleRate = 44100.0;
    float m_lfoPhase = 0.0f;
    float m_rate = 0.5f;
    float m_feedback = 0.3f;

    std::vector<float> m_filterState[2] = { std::vector<float>(4, 0.0f), std::vector<float>(4, 0.0f) };
    float m_lastOut[2] = {0, 0};
};

} // namespace Aura::DSP::Effects
