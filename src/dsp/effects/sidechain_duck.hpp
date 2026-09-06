#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include <atomic>
#include <cstring>
#include <cstdio>
#include "../iprocessor.hpp"
#include "../../core/engine/bus_system.hpp"

namespace Aura::DSP::Effects {

/**
 * @class SidechainDuck
 * @brief Dynamic Pumping effect for professional Electronic/Trap music.
 * HONEST FIX: Uses external sidechain bus or internal LFO (Sync'd to BPM).
 * Provides 'The Bounce' found in modern Logic Pro productions.
 */
class SidechainDuck : public IProcessor {
public:
    SidechainDuck() = default;

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        m_sampleRate = std::isfinite(sr) && sr > 1000.0 ? sr : 44100.0;
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        const uint32_t n = buffer.getNumSamples();
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!left || n == 0) return;
        const Core::AudioBuffer* sidechain = context.sidechainBuffer;
        const double bpm = std::isfinite(context.bpm) && context.bpm > 1.0 ? context.bpm : 120.0;
        const double lfoHz = bpm / 60.0;
        const float depthValue = m_depth.load(std::memory_order_relaxed);
        const float depth = std::clamp(std::isfinite(depthValue) ? depthValue : 0.0f, 0.0f, 1.0f);
        const float attack = std::exp(-1.0f / (0.005f * static_cast<float>(m_sampleRate)));
        const float release = std::exp(-1.0f / (0.080f * static_cast<float>(m_sampleRate)));
        for (uint32_t i = 0; i < n; ++i) {
            float detector = 0.0f;
            if (sidechain && sidechain->getNumChannels() > 0 && i < sidechain->getNumSamples()) {
                detector = std::abs(sidechain->getReadPointer(0)[i]);
                if (sidechain->getNumChannels() > 1) detector = std::max(detector, std::abs(sidechain->getReadPointer(1)[i]));
            } else {
                detector = 0.5f + 0.5f * std::sin(2.0 * M_PI * m_lfoPhase);
                m_lfoPhase += lfoHz / m_sampleRate;
                if (m_lfoPhase >= 1.0) m_lfoPhase -= std::floor(m_lfoPhase);
            }
            const float target = 1.0f - depth * std::clamp(detector, 0.0f, 1.0f);
            m_currentGain = target < m_currentGain ? attack * m_currentGain + (1.0f - attack) * target
                                                     : release * m_currentGain + (1.0f - release) * target;
            left[i] = std::isfinite(left[i] * m_currentGain) ? left[i] * m_currentGain : 0.0f;
            if (right) right[i] = std::isfinite(right[i] * m_currentGain) ? right[i] * m_currentGain : 0.0f;
        }
    }


    void reset() noexcept override {
        m_currentGain = 1.0f;
        m_lfoPhase = 0.0;
    }

    void setDepth(float d) noexcept { m_depth.store(std::clamp(d, 0.0f, 1.0f), std::memory_order_relaxed); }
    std::string getName() const override { return "Sidechain Duck"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0 && std::isfinite(value)) setDepth(value); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_depth.load(std::memory_order_relaxed) : 0.0f; }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id != 0) return false; out = {0.0f, 1.0f, false}; return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (outName && maxSize > 0) std::snprintf(outName, maxSize, "%s", id == 0 ? "Duck Depth" : "");
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(24, 0); const uint32_t magic = 0x41555241u; const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u); const float depth = getParameter(0);
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data()+4, &version, 2); std::memcpy(state.data()+6, &flags, 2);
        std::memcpy(state.data()+8, &m_mix, 4); std::memcpy(state.data()+12, &m_sidechainBusId, 4); std::memcpy(state.data()+16, &depth, 4);
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 24) return false;
        uint32_t magic = 0, sidechain = 0; uint16_t version = 0, flags = 0; float mix = 0.0f, depth = 0.0f;
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data()+4, 2); std::memcpy(&flags, state.data()+6, 2);
        std::memcpy(&mix, state.data()+8, 4); std::memcpy(&sidechain, state.data()+12, 4); std::memcpy(&depth, state.data()+16, 4);
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f || !std::isfinite(depth) || depth < 0.0f || depth > 1.0f) return false;
        m_bypassed = (flags & 1u) != 0; m_mix = mix; m_sidechainBusId = sidechain; setDepth(depth); return true;
    }

private:
    double m_sampleRate = 44100.0;
    std::atomic<float> m_depth{0.8f};
    float m_currentGain = 1.0f;
    double m_lfoPhase = 0.0;
};

} // namespace Aura::DSP::Effects
