#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include <limits>
#include <cstring>
#include <cstdio>
#include "../iprocessor.hpp"
#include "../effects/delay_line.hpp"

namespace Aura::DSP::Effects {

/**
 * @brief VocalDoubler: Industrial-standard vocal thickening.
 * Creates 'Double' takes automatically with micro-timing and pitch shifts.
 */
class VocalDoubler : public IProcessor {
public:
    VocalDoubler(double sr = 44100.0) : m_sampleRate(44'100.0) { setMix(0.5f); setSampleRate(sr); }

    std::string getName() const override { return "Vocal Doubler"; }
    uint32_t getNumParameters() const noexcept override { return 2; }
    void setParameter(uint32_t id, float value) noexcept override { if (!std::isfinite(value)) return; if (id == 0) setMix(value); else if (id == 1) setDepth(value); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_mix : (id == 1 ? m_depth : 0.0f); }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override { if (id >= 2) return false; out = {0.0f, 1.0f, false}; return true; }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override { if (outName && maxSize > 0) std::snprintf(outName, maxSize, "%s", id == 0 ? "Mix" : (id == 1 ? "Depth" : "")); }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44'100.0;
        m_delayL = DelayLine(static_cast<uint32_t>(m_sampleRate * 0.1) + 2u);
        m_delayR = DelayLine(static_cast<uint32_t>(m_sampleRate * 0.1) + 2u);
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        if (m_bypassed || buffer.getNumChannels() == 0) return;
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
            const float outL = dryL + mix * depth * 0.55f * (doubledL - dryL);
            const float outR = dryR + mix * depth * 0.55f * (doubledR - dryR);
            if (r == l) l[i] = std::isfinite(0.5f * (outL + outR)) ? 0.5f * (outL + outR) : 0.0f;
            else { l[i] = std::isfinite(outL) ? outL : 0.0f; r[i] = std::isfinite(outR) ? outR : 0.0f; }
            m_phase += 0.2 / sr;
            if (m_phase >= 1.0) m_phase -= std::floor(m_phase);
        }
    }

    void reset() noexcept override {
        m_delayL.reset();
        m_delayR.reset();
        m_phase = 0.0;
    }

    void setSampleRate(double sr) {
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44'100.0;
        const uint32_t capacity = static_cast<uint32_t>(m_sampleRate * 0.1) + 2u;
        m_delayL = DelayLine(capacity);
        m_delayR = DelayLine(capacity);
        reset();
    }
    void setMix(float mix) noexcept { if (std::isfinite(mix)) m_mix = std::clamp(mix, 0.0f, 1.0f); }
    void setDepth(float depth) noexcept { if (std::isfinite(depth)) m_depth = std::clamp(depth, 0.0f, 1.0f); }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(32, 0); const uint32_t magic = 0x41555241u; const uint16_t version = 1; const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        const float values[] = {m_mix, m_depth}; std::memcpy(state.data(), &magic, 4); std::memcpy(state.data()+4, &version, 2); std::memcpy(state.data()+6, &flags, 2); std::memcpy(state.data()+8, &m_sidechainBusId, 4); std::memcpy(state.data()+12, values, sizeof(values)); return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 32) return false; uint32_t magic = 0, sidechain = 0; uint16_t version = 0, flags = 0; float values[2]{};
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data()+4, 2); std::memcpy(&flags, state.data()+6, 2); std::memcpy(&sidechain, state.data()+8, 4); std::memcpy(values, state.data()+12, sizeof(values));
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(values[0]) || !std::isfinite(values[1]) || values[0] < 0.0f || values[0] > 1.0f || values[1] < 0.0f || values[1] > 1.0f) return false;
        m_bypassed = (flags & 1u) != 0; m_sidechainBusId = sidechain; setMix(values[0]); setDepth(values[1]); return true;
    }
    uint32_t getLatencySamples() const noexcept override { return 0; }
    uint32_t getTailSamples() const noexcept override {
        return static_cast<uint32_t>(std::min(0.1 * std::max(1.0, m_sampleRate),
                                              static_cast<double>(std::numeric_limits<uint32_t>::max())));
    }

private:
    double m_sampleRate;
    double m_phase = 0.0;
    float m_depth = 1.0f;
    DelayLine m_delayL{44102};
    DelayLine m_delayR{44102};
};

} // namespace Aura::DSP::Effects
