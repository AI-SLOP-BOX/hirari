#pragma once
#include <cmath>
#include <algorithm>
#include <vector>
#include <cstring>
#include <cstdio>
#include <atomic>
#include "../../core/audio_buffer.hpp"
#include "../../core/midi_buffer.hpp"
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class SidechainCompressor
 * @brief Professional High-performance Ducking Engine using the Sidechain input.
 * HONEST FIX: Uses context.sidechainBuffer to drive the gain reduction (Duck).
 */
class SidechainCompressor : public IProcessor {
public:
    SidechainCompressor() : m_threshold(0.2f), m_ratio(10.0f), m_attack(10.0f), m_release(100.0f) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        if (std::isfinite(sr) && sr >= 100.0 && sr <= 384000.0) m_sampleRate = sr;
    }

    uint32_t getTailSamples() const noexcept override {
        const double sr = std::clamp(m_sampleRate, 100.0, 384000.0);
        const float release = m_release.load(std::memory_order_relaxed);
        const double ms = std::clamp(std::isfinite(release) ? release : 100.0f, 1.0f, 5000.0f);
        return static_cast<uint32_t>(std::min(30.0 * sr, ms * 0.001 * sr * 8.0));
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const ProcessContext& context) noexcept override {
        if (m_bypassed) return;
        const uint32_t samples = buffer.getNumSamples();
        if (samples == 0 || buffer.getNumChannels() == 0) return;
        const float* scL = nullptr;
        const float* scR = nullptr;
        if (context.sidechainBuffer &&
            context.sidechainBuffer->getNumSamples() >= samples &&
            context.sidechainBuffer->getNumChannels() > 0) {
            scL = context.sidechainBuffer->getReadPointer(0);
            scR = context.sidechainBuffer->getNumChannels() > 1
                ? context.sidechainBuffer->getReadPointer(1) : scL;
        }
        const float* mainL = buffer.getReadPointer(0);
        const float* mainR = buffer.getNumChannels() > 1 ? buffer.getReadPointer(1) : mainL;
        float* outL = buffer.getWritePointer(0);
        float* outR = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : outL;
        if (!mainL || !mainR || !outL || !outR) return;
        const double sr = (std::isfinite(context.sampleRate) && context.sampleRate > 1.0)
            ? context.sampleRate : m_sampleRate;
        const float attackCoeff = 1.0f - std::exp(-1.0f /
            static_cast<float>(std::max(1.0, static_cast<double>(m_attack.load(std::memory_order_relaxed))) * 0.001 * sr));
        const float releaseCoeff = 1.0f - std::exp(-1.0f /
            static_cast<float>(std::max(1.0, static_cast<double>(m_release.load(std::memory_order_relaxed))) * 0.001 * sr));
        const float thresholdValue = m_threshold.load(std::memory_order_relaxed);
        const float ratioValue = m_ratio.load(std::memory_order_relaxed);
        const float threshold = std::clamp(std::isfinite(thresholdValue) ? thresholdValue : 0.2f,
                                           1.0e-5f, 1.0f);
        const float ratio = std::max(1.0f, std::isfinite(ratioValue) ? ratioValue : 1.0f);
        for (uint32_t i = 0; i < samples; ++i) {
            const float detectorL = scL ? scL[i] : mainL[i];
            const float detectorR = scR ? scR[i] : mainR[i];
            const float detector = std::max(std::abs(detectorL), std::abs(detectorR));
            const float targetEnv = std::isfinite(detector) ? detector : 0.0f;
            const float envCoeff = targetEnv > m_env ? attackCoeff : releaseCoeff;
            m_env += (targetEnv - m_env) * envCoeff;
            float desiredGain = 1.0f;
            if (m_env > threshold) {
                const float compressed = threshold + (m_env - threshold) / ratio;
                desiredGain = std::clamp(compressed / std::max(m_env, 1.0e-6f), 0.0f, 1.0f);
            }
            const float gainCoeff = desiredGain < m_currentGain ? attackCoeff : releaseCoeff;
            m_currentGain += (desiredGain - m_currentGain) * gainCoeff;
            outL[i] = std::isfinite(mainL[i] * m_currentGain) ? mainL[i] * m_currentGain : 0.0f;
            if (outR != outL) {
                outR[i] = std::isfinite(mainR[i] * m_currentGain) ? mainR[i] * m_currentGain : 0.0f;
            }
        }
    }


    void reset() noexcept override {
        m_env = 0.0f;
        m_currentGain = 1.0f;
    }

    // Parameters
    void setThreshold(float t) noexcept { m_threshold.store(std::clamp(t, 1.0e-5f, 1.0f), std::memory_order_relaxed); }
    void setRatio(float r) noexcept { m_ratio.store(std::clamp(r, 1.0f, 100.0f), std::memory_order_relaxed); }
    void setAttack(float ms) noexcept { m_attack.store(std::clamp(ms, 0.1f, 5000.0f), std::memory_order_relaxed); }
    void setRelease(float ms) noexcept { m_release.store(std::clamp(ms, 1.0f, 5000.0f), std::memory_order_relaxed); }

    std::string getName() const override { return "Sidechain Compressor"; }
    uint32_t getNumParameters() const noexcept override { return 4; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        switch (id) {
            case 0: setThreshold(value); break;
            case 1: setRatio(1.0f + value * 99.0f); break;
            case 2: setAttack(0.1f + value * 4999.9f); break;
            case 3: setRelease(1.0f + value * 4999.0f); break;
            default: break;
        }
    }
    float getParameter(uint32_t id) const noexcept override {
        switch (id) {
            case 0: return m_threshold.load(std::memory_order_relaxed);
            case 1: return (m_ratio.load(std::memory_order_relaxed) - 1.0f) / 99.0f;
            case 2: return (m_attack.load(std::memory_order_relaxed) - 0.1f) / 4999.9f;
            case 3: return (m_release.load(std::memory_order_relaxed) - 1.0f) / 4999.0f;
            default: return 0.0f;
        }
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id > 3) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        const char* names[] = {"Threshold", "Ratio", "Attack", "Release"};
        std::snprintf(outName, maxSize, "%s", id < 4 ? names[id] : "");
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(32, 0);
        const uint32_t magic = 0x41555241u; const uint16_t version = 1;
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data()+4, &version, 2);
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        std::memcpy(state.data()+6, &flags, 2); std::memcpy(state.data()+8, &m_mix, 4);
        std::memcpy(state.data()+12, &m_sidechainBusId, 4);
        const float values[] = {getParameter(0), getParameter(1), getParameter(2), getParameter(3)};
        std::memcpy(state.data()+16, values, sizeof(values));
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 32) return false;
        uint32_t magic = 0; uint16_t version = 0; std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data()+4, 2);
        if (magic != 0x41555241u || version != 1) return false;
        uint16_t flags = 0; float mix = 0.0f; uint32_t sidechain = 0;
        std::memcpy(&flags, state.data()+6, 2); std::memcpy(&mix, state.data()+8, 4); std::memcpy(&sidechain, state.data()+12, 4);
        if ((flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f) return false;
        float values[4]{}; std::memcpy(values, state.data()+16, sizeof(values));
        for (float v : values) if (!std::isfinite(v) || v < 0.0f || v > 1.0f) return false;
        for (uint32_t i = 0; i < 4; ++i) setParameter(i, values[i]);
        m_bypassed = (flags & 1u) != 0; m_mix = mix; m_sidechainBusId = sidechain;
        return true;
    }

private:
    double m_sampleRate = 44100.0;
    std::atomic<float> m_threshold, m_ratio, m_attack, m_release;
    float m_env = 0.0f;
    float m_currentGain = 1.0f;
};

} // namespace Aura::DSP::Effects
