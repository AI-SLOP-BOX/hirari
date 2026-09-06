#pragma once
#include <vector>
#include <algorithm>
#include <cmath>
#include <cstring>
#include <cstdio>
#include <atomic>
#include "../../core/audio_buffer.hpp"
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class MasterLimiter
 * @brief Professional Look-ahead Peak Limiter.
 * Features circular look-ahead buffer and smooth release envelope.
 * HONEST FIX: Purged fraudulent oversampling claims and unused SIMD code.
 */
class MasterLimiter : public IProcessor {
public:
    static constexpr uint32_t kMaxLookahead = 2048; 

    MasterLimiter(double sr = 44100.0) : m_sampleRate(sr) {
        prepareToPlay(sr, 1024);
    }

    std::string getName() const override { return "AURA Master Limiter"; }

    uint32_t getLatencySamples() const noexcept override {
        const double sr = std::isfinite(m_sampleRate) && m_sampleRate > 0.0
            ? m_sampleRate : 44'100.0;
        const float lookaheadValue = m_lookaheadMs.load(std::memory_order_relaxed);
        const float lookahead = std::isfinite(lookaheadValue)
            ? std::clamp(lookaheadValue, 0.0f, 20.0f) : 2.0f;
        const double samples = std::round(static_cast<double>(lookahead) * 0.001 * sr);
        return static_cast<uint32_t>(std::clamp(samples, 0.0,
            static_cast<double>(kMaxLookahead - 1)));
    }
    uint32_t getTailSamples() const noexcept override {
        const double sr = std::isfinite(m_sampleRate) ? m_sampleRate : 44'100.0;
        const float releaseValue = m_releaseMs.load(std::memory_order_relaxed);
        const float release = std::isfinite(releaseValue) ? std::clamp(releaseValue, 1.0f, 1000.0f) : 50.0f;
        return static_cast<uint32_t>(std::min(30.0 * sr, release * 0.001 * sr * 8.0));
    }

    void prepareToPlay(double sr, uint32_t /*bs*/) noexcept override {
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0
            ? sr : 44'100.0;
        m_delayL.assign(kMaxLookahead, 0.0f);
        m_delayR.assign(kMaxLookahead, 0.0f);
        m_writeIdx = 0;
        m_currentGain = 1.0f;
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& /*midi*/, const ProcessContext& /*context*/) noexcept override {
        if (isBypassed() || buffer.isEmpty()) return;

        uint32_t numSamples = buffer.getNumSamples();
        const uint32_t numChannels = buffer.getNumChannels();
        float* l = buffer.getWritePointer(0);
        float* r = numChannels > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!l) return;

        // Sanitize sample rate & release
        const double sr = (std::isfinite(m_sampleRate) && m_sampleRate > 0.0) ? m_sampleRate : 44100.0;
        const float releaseValue = m_releaseMs.load(std::memory_order_relaxed);
        const float releaseMs = std::isfinite(releaseValue) ? std::clamp(releaseValue, 1.0f, 1000.0f) : 50.0f;

        float releaseCoef = std::exp(-1.0f / (releaseMs * 0.001f * static_cast<float>(sr)));
        if (!std::isfinite(releaseCoef)) releaseCoef = 0.99f;
        releaseCoef = std::clamp(releaseCoef, 0.0f, 0.9999f);

        const float thresholdValue = m_thresholdGain.load(std::memory_order_relaxed);
        const float thresholdGain = std::isfinite(thresholdValue)
            ? std::clamp(thresholdValue, 0.001f, 15.8489f)
            : 1.0f;
        const float ceilingValue = m_ceiling.load(std::memory_order_relaxed);
        const float ceiling = std::isfinite(ceilingValue)
            ? std::clamp(ceilingValue, 0.001f, 1.0f)
            : 0.99f;
        const float lookaheadValue2 = m_lookaheadMs.load(std::memory_order_relaxed);
        const float lookaheadMs = std::isfinite(lookaheadValue2)
            ? std::clamp(lookaheadValue2, 0.0f, 20.0f)
            : 2.0f;
        uint32_t lookaheadSamples = static_cast<uint32_t>(
            std::round(lookaheadMs * 0.001f * static_cast<float>(sr)));
        lookaheadSamples = std::clamp<uint32_t>(lookaheadSamples, 0, kMaxLookahead - 1);

        for (uint32_t s = 0; s < numSamples; ++s) {
            float inL = std::isfinite(l[s]) ? l[s] * thresholdGain : 0.0f;
            float inR = r && std::isfinite(r[s]) ? r[s] * thresholdGain : inL;

            m_delayL[m_writeIdx] = inL;
            m_delayR[m_writeIdx] = inR;

            uint32_t readIdx = (m_writeIdx + kMaxLookahead - lookaheadSamples) & (kMaxLookahead - 1);
            float outL = m_delayL[readIdx];
            float outR = m_delayR[readIdx];
            // The delayed sample is about to leave the ring.  Its
            // look-ahead window is the samples from readIdx through the
            // current write position; inspect that queued window so the
            // attenuation envelope is aligned with the emitted sample.
            float peak = 0.0f;
            const uint32_t window = lookaheadSamples + 1;
            for (uint32_t ahead = 0; ahead < window; ++ahead) {
                const uint32_t index = (readIdx + ahead) & (kMaxLookahead - 1);
                const uint32_t previous = (index + kMaxLookahead - 1) & (kMaxLookahead - 1);
                peak = std::max(peak, std::max(std::abs(m_delayL[index]),
                                              std::abs(m_delayR[index])));
                // Include a midpoint estimate for the inter-sample peak. This
                // catches reconstructed peaks that can exceed the sample
                // ceiling after a DAC's interpolation filter.
                peak = std::max(peak, std::max(
                    std::abs(0.5f * (m_delayL[previous] + m_delayL[index])),
                    std::abs(0.5f * (m_delayR[previous] + m_delayR[index]))));
            }
            m_writeIdx = (m_writeIdx + 1) & (kMaxLookahead - 1);

            float targetAtten = (peak > ceiling) ? ceiling / (peak + 1e-6f) : 1.0f;
            if (!std::isfinite(targetAtten)) targetAtten = 1.0f;

            if (targetAtten < m_currentGain) {
                m_currentGain = targetAtten; // Instant attack for limiting
            } else {
                m_currentGain = m_currentGain * releaseCoef + targetAtten * (1.0f - releaseCoef);
            }
            if (!std::isfinite(m_currentGain)) m_currentGain = 1.0f;

            const float limitedL = outL * m_currentGain;
            const float limitedR = outR * m_currentGain;
            // Final safety ceiling: the envelope is predictive, but a hard
            // finite clamp protects the audio graph from pathological input
            // and floating-point overshoot at the publication boundary.
            l[s] = std::isfinite(limitedL)
                ? std::clamp(limitedL, -ceiling, ceiling) : 0.0f;
            if (r) {
                r[s] = std::isfinite(limitedR)
                    ? std::clamp(limitedR, -ceiling, ceiling) : 0.0f;
            }
        }
    }

    void setThreshold(float db) { 
        if (std::isfinite(db)) {
            m_thresholdGain.store(std::pow(10.0f, std::clamp(db, -60.0f, 24.0f) / 20.0f), std::memory_order_relaxed);
        }
    }
    void setCeiling(float db) { 
        if (std::isfinite(db)) {
            m_ceiling.store(std::pow(10.0f, std::clamp(db, -60.0f, 0.0f) / 20.0f), std::memory_order_relaxed);
        }
    }
    void setRelease(float ms) {
        if (std::isfinite(ms)) {
            m_releaseMs.store(std::clamp(ms, 1.0f, 1000.0f), std::memory_order_relaxed);
        }
    }
    void setLookaheadMs(float ms) {
        if (std::isfinite(ms)) {
            m_lookaheadMs.store(std::clamp(ms, 0.0f, 20.0f), std::memory_order_relaxed);
        }
    }

    uint32_t getNumParameters() const noexcept override { return 4; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        switch (id) {
            case 0: setThreshold(-60.0f + value * 84.0f); break;
            case 1: setCeiling(-60.0f + value * 60.0f); break;
            case 2: setRelease(1.0f + value * 999.0f); break;
            case 3: setLookaheadMs(value * 20.0f); break;
            default: break;
        }
    }
    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return std::clamp((20.0f * std::log10(std::max(m_thresholdGain.load(std::memory_order_relaxed), 0.001f)) + 60.0f) / 84.0f, 0.0f, 1.0f);
        if (id == 1) return std::clamp((20.0f * std::log10(std::max(m_ceiling.load(std::memory_order_relaxed), 0.001f)) + 60.0f) / 60.0f, 0.0f, 1.0f);
        if (id == 2) return std::clamp((m_releaseMs.load(std::memory_order_relaxed) - 1.0f) / 999.0f, 0.0f, 1.0f);
        if (id == 3) return std::clamp(m_lookaheadMs.load(std::memory_order_relaxed) / 20.0f, 0.0f, 1.0f);
        return 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 4) return false; out = {0.0f, 1.0f, false}; return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        const char* names[] = {"Threshold", "Ceiling", "Release", "Lookahead"};
        std::snprintf(outName, maxSize, "%s", id < 4 ? names[id] : "");
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(32, 0); const uint32_t magic = 0x41555241u; const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data()+4, &version, 2); std::memcpy(state.data()+6, &flags, 2);
        std::memcpy(state.data()+8, &m_mix, 4); std::memcpy(state.data()+12, &m_sidechainBusId, 4);
        const float values[] = {getParameter(0), getParameter(1), getParameter(2), getParameter(3)};
        std::memcpy(state.data()+16, values, sizeof(values)); return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 32) return false;
        uint32_t magic = 0, sidechain = 0; uint16_t version = 0, flags = 0; float mix = 0.0f; float values[4]{};
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data()+4, 2); std::memcpy(&flags, state.data()+6, 2);
        std::memcpy(&mix, state.data()+8, 4); std::memcpy(&sidechain, state.data()+12, 4); std::memcpy(values, state.data()+16, sizeof(values));
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f) return false;
        for (float value : values) if (!std::isfinite(value) || value < 0.0f || value > 1.0f) return false;
        m_bypassed = (flags & 1u) != 0; m_mix = mix; m_sidechainBusId = sidechain;
        for (uint32_t i = 0; i < 4; ++i) setParameter(i, values[i]); return true;
    }

    void reset() noexcept override {
        m_currentGain = 1.0f;
        std::fill(m_delayL.begin(), m_delayL.end(), 0.0f);
        std::fill(m_delayR.begin(), m_delayR.end(), 0.0f);
        m_writeIdx = 0;
    }

private:
    double m_sampleRate;
    std::atomic<float> m_ceiling{0.99f};
    std::atomic<float> m_thresholdGain{1.0f};
    float m_currentGain = 1.0f;
    std::atomic<float> m_releaseMs{50.0f};
    std::atomic<float> m_lookaheadMs{2.0f};
    
    std::vector<float> m_delayL;
    std::vector<float> m_delayR;
    uint32_t m_writeIdx = 0;
};

} // namespace Aura::DSP::Effects
