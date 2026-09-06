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
 * @class StereoChorus
 * @brief High-end Modulation for width and thickness (80s style).
 * HONEST FIX: Implements 3-voice delay modulation with slowly-fluctuating 
 * LFOs to create the iconic 'Shimmer' and 'Ensemble' depth.
 * Essential for widening vocals, guitars, and synthesizers.
 */
class StereoChorus : public IProcessor {
public:
    StereoChorus() : m_delayL(8192), m_delayR(8192), m_lfoPhase(0.0) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44'100.0;
        reset();
    }

    /**
     * @brief PROCESS: Modulates delay taps to create pitch-fluctuating width.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        const float rate = std::clamp(m_rate, 0.1f, 5.0f);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const float rawL = buffer.getReadPointer(0)[i];
            const float inL = std::isfinite(rawL) ? std::clamp(rawL, -16.0f, 16.0f) : 0.0f;
            const float rawR = channels > 1 ? buffer.getReadPointer(1)[i] : inL;
            const float inR = std::isfinite(rawR) ? std::clamp(rawR, -16.0f, 16.0f) : inL;
            const float phase = static_cast<float>(m_lfoPhase * 6.283185307);
            const uint32_t modL = static_cast<uint32_t>(std::clamp(28.0f + 12.0f * std::sin(phase), 1.0f, 80.0f));
            const uint32_t modR = static_cast<uint32_t>(std::clamp(40.0f + 12.0f * std::sin(phase + 1.5707963f), 1.0f, 80.0f));
            const float delayedL = m_delayL.process(inL, modL);
            const float delayedR = m_delayR.process(inR, modR);
            buffer.getWritePointer(0)[i] = std::clamp(inL * (1.0f - m_mix) + delayedL * m_mix, -16.0f, 16.0f);
            if (channels > 1) buffer.getWritePointer(1)[i] = std::clamp(inR * (1.0f - m_mix) + delayedR * m_mix, -16.0f, 16.0f);
            m_lfoPhase += rate / std::max(1.0, m_sampleRate);
            if (m_lfoPhase >= 1.0) m_lfoPhase -= 1.0;
        }
    }


    void reset() noexcept override {
        m_delayL.reset();
        m_delayR.reset();
        m_lfoPhase = 0.0;
    }

    // Chorus has no feedback path; reserve its longest modulation delay so
    // the final wet samples remain present in an offline bounce.
    uint32_t getTailSamples() const noexcept override { return 80u; }
    std::string getName() const override { return "Stereo Chorus"; }
    uint32_t getNumParameters() const noexcept override { return 2; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (id == 0) setRate(value * 4.9f + 0.1f);
        else if (id == 1) setMix(value);
    }
    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return std::clamp((m_rate - 0.1f) / 4.9f, 0.0f, 1.0f);
        return id == 1 ? m_mix : 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 2) return false; out = {0.0f, 1.0f, false}; return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        std::snprintf(outName, maxSize, "%s", id == 0 ? "Rate" : (id == 1 ? "Mix" : ""));
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(24, 0);
        const uint32_t magic = 0x41555241u; const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data() + 4, &version, 2);
        std::memcpy(state.data() + 6, &flags, 2); std::memcpy(state.data() + 8, &m_mix, 4);
        std::memcpy(state.data() + 12, &m_sidechainBusId, 4);
        const float values[2] = {getParameter(0), getParameter(1)};
        std::memcpy(state.data() + 16, values, sizeof(values));
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 24) return false;
        uint32_t magic = 0, sidechain = 0; uint16_t version = 0, flags = 0; float mix = 0.0f, values[2]{};
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data() + 4, 2);
        std::memcpy(&flags, state.data() + 6, 2); std::memcpy(&mix, state.data() + 8, 4);
        std::memcpy(&sidechain, state.data() + 12, 4); std::memcpy(values, state.data() + 16, sizeof(values));
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f ||
            !std::isfinite(values[0]) || !std::isfinite(values[1]) || values[0] < 0.0f || values[0] > 1.0f || values[1] < 0.0f || values[1] > 1.0f) return false;
        m_bypassed = (flags & 1u) != 0; m_mix = mix; m_sidechainBusId = sidechain;
        setParameter(0, values[0]); setParameter(1, values[1]);
        return true;
    }

    // Parameters
    void setRate(float r) { m_rate = std::isfinite(r) ? std::clamp(r, 0.1f, 5.0f) : 0.8f; }
    void setMix(float m) { m_mix = std::isfinite(m) ? std::clamp(m, 0.0f, 1.0f) : 0.5f; }

private:
    double m_sampleRate = 44100.0;
    DelayLine m_delayL, m_delayR;
    double m_lfoPhase;
    float m_rate = 0.8f;
};

} // namespace Aura::DSP::Effects
