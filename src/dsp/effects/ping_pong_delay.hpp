#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include <cstdio>
#include <cstring>
#include "../iprocessor.hpp"
#include "delay_line.hpp"

namespace Aura::DSP::Effects {

/**
 * @class PingPongDelay
 * @brief High-end Rhythmic Ping-Pong Delay with BPM Sync.
 * HONEST FIX: Implements a cross-feedback delay loop where the 
 * feedback of the Left channel is routed to the Right and vice-versa.
 * Creates the immersive rhythmic width found in professional Logic Pro 
 * Delay Designer presets.
 */
class PingPongDelay : public IProcessor {
public:
    PingPongDelay() : m_delayL(65536), m_delayR(65536) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0
            ? sr : 44'100.0;
        reset();
    }

    /**
     * @brief PROCESS: Cross-feedback stereo delay loop.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        const double bpm = std::isfinite(context.bpm) && context.bpm > 1.0 ? context.bpm : 120.0;
        const uint32_t delaySamples = static_cast<uint32_t>(std::clamp(
            m_sampleRate * (60.0 / bpm) * std::max(0.0625f, m_noteValue * 4.0f), 1.0, 65535.0));
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const float rawL = buffer.getReadPointer(0)[i];
            const float inL = std::isfinite(rawL) ? std::clamp(rawL, -16.0f, 16.0f) : 0.0f;
            const float rawR = channels > 1 ? buffer.getReadPointer(1)[i] : inL;
            const float inR = std::isfinite(rawR) ? std::clamp(rawR, -16.0f, 16.0f) : inL;
            const float delayedL = m_delayL.process(inL + m_lastOutR * m_feedbackR, delaySamples);
            const float delayedR = m_delayR.process(inR + m_lastOutL * m_feedbackL, delaySamples);
            m_lastOutL = delayedL;
            m_lastOutR = delayedR;
            const float outL = inL * (1.0f - m_mix) + delayedL * m_mix;
            const float outR = inR * (1.0f - m_mix) + delayedR * m_mix;
            buffer.getWritePointer(0)[i] = std::isfinite(outL) ? std::clamp(outL, -16.0f, 16.0f) : 0.0f;
            if (channels > 1) buffer.getWritePointer(1)[i] = std::isfinite(outR) ? std::clamp(outR, -16.0f, 16.0f) : 0.0f;
        }
    }


    void reset() noexcept override {
        m_delayL.reset();
        m_delayR.reset();
        m_lastOutL = 0.0f;
        m_lastOutR = 0.0f;
    }

    uint32_t getTailSamples() const noexcept override {
        const double rate = std::isfinite(m_sampleRate) && m_sampleRate > 0.0
            ? m_sampleRate : 44'100.0;
        const double tail = std::min(30.0 * rate, 65535.0 * 128.0);
        return static_cast<uint32_t>(tail);
    }

    // Parameters
    void setNoteValue(float v) { m_noteValue = std::isfinite(v) ? std::clamp(v, 0.0625f, 4.0f) : 0.25f; }
    void setFeedback(float f) { const float safe = std::isfinite(f) ? std::clamp(f, 0.0f, 0.99f) : 0.5f; m_feedbackL = m_feedbackR = safe; }
    void setMix(float m) { m_mix = std::isfinite(m) ? std::clamp(m, 0.0f, 1.0f) : 0.5f; }
    std::string getName() const override { return "Ping Pong Delay"; }
    uint32_t getNumParameters() const noexcept override { return 3; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (id == 0) setNoteValue(0.0625f + std::clamp(value, 0.0f, 1.0f) * 3.9375f);
        else if (id == 1) setFeedback(value);
        else if (id == 2) setMix(value);
    }
    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return std::clamp((m_noteValue - 0.0625f) / 3.9375f, 0.0f, 1.0f);
        if (id == 1) return m_feedbackL;
        return id == 2 ? m_mix : 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 3) return false; out = {0.0f, 1.0f, false}; return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        const char* names[] = {"Note Value", "Feedback", "Mix"};
        std::snprintf(outName, maxSize, "%s", id < 3 ? names[id] : "");
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(28, 0);
        const uint32_t magic = 0x41555241u; const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data() + 4, &version, 2);
        std::memcpy(state.data() + 6, &flags, 2); std::memcpy(state.data() + 8, &m_mix, 4);
        std::memcpy(state.data() + 12, &m_sidechainBusId, 4);
        const float values[3] = {getParameter(0), getParameter(1), getParameter(2)};
        std::memcpy(state.data() + 16, values, sizeof(values));
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

private:
    double m_sampleRate = 44100.0;
    DelayLine m_delayL, m_delayR;
    float m_lastOutL = 0.0f;
    float m_lastOutR = 0.0f;
    
    float m_noteValue = 0.25f; // Quarter note sync
    float m_feedbackL = 0.5f;
    float m_feedbackR = 0.5f;
};

} // namespace Aura::DSP::Effects
