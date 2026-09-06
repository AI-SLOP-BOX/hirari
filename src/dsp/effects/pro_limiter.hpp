#pragma once

#include <cmath>
#include <algorithm>
#include <array>
#include <cstring>
#include <atomic>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class ProLimiter
 * @brief Professional Brickwall Lookahead Limiter.
 */
class ProLimiter : public IProcessor {
public:
    static constexpr uint32_t kLookahead = 480; 

    ProLimiter(double sr = 44100.0) : m_sampleRate(sr) {
        m_delayL.fill(0.0f);
        m_delayR.fill(0.0f);
        reset();
    }

    void prepareToPlay(double sr, uint32_t /*bs*/) noexcept override {
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0
            ? sr : 44'100.0;
        m_release = std::exp(-1.0f / (0.05f * static_cast<float>(m_sampleRate))); // 50ms default
    }

    uint32_t getLatencySamples() const noexcept override { return kLookahead; }
    uint32_t getTailSamples() const noexcept override {
        const double rate = std::isfinite(m_sampleRate) && m_sampleRate > 0.0
            ? m_sampleRate : 44'100.0;
        // Seven release time constants leave the gain envelope below -60 dB.
        const double tail = static_cast<double>(kLookahead) + 0.35 * rate;
        return static_cast<uint32_t>(std::min<double>(tail, 30.0 * rate));
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& /*midi*/, const ProcessContext& /*context*/) noexcept override {
        if (isBypassed() || buffer.getNumChannels() < 2 || buffer.getNumSamples() == 0) return;
        uint32_t numSamples = buffer.getNumSamples();
        float* l = buffer.getWritePointer(0);
        float* r = buffer.getWritePointer(1);

        for (uint32_t s = 0; s < numSamples; ++s) {
            float inL = std::isfinite(l[s]) ? l[s] : 0.0f;
            float inR = std::isfinite(r[s]) ? r[s] : 0.0f;
            
            // 1. Peak Detection (Link)
            float peak = std::max(std::abs(inL), std::abs(inR));
            const float threshold = m_threshold.load(std::memory_order_relaxed);
            float targetGain = (peak > threshold) ? (threshold / peak) : 1.0f;

            // 2. Lookahead Ballistics (Attack must be faster than lookahead)
            if (targetGain < m_envelope) {
                m_envelope = targetGain; // Instant attack on peak
            } else {
                m_envelope = targetGain + m_release * (m_envelope - targetGain);
            }

            // 3. Apply to delayed signal
            uint32_t readIdx = (m_writeIdx - kLookahead + m_delayL.size()) % m_delayL.size();
            l[s] = std::isfinite(m_delayL[readIdx] * m_envelope)
                ? m_delayL[readIdx] * m_envelope : 0.0f;
            r[s] = std::isfinite(m_delayR[readIdx] * m_envelope)
                ? m_delayR[readIdx] * m_envelope : 0.0f;

            // 4. Update Delay Lines
            m_delayL[m_writeIdx] = inL;
            m_delayR[m_writeIdx] = inR;
            m_writeIdx = (m_writeIdx + 1) % m_delayL.size();
        }
    }

    void reset() noexcept override {
        m_envelope = 1.0f;
        m_writeIdx = 0;
        m_delayL.fill(0.0f);
        m_delayR.fill(0.0f);
    }

    void setThreshold(float db) noexcept { if (std::isfinite(db)) m_threshold.store(std::pow(10.0f, db / 20.0f), std::memory_order_relaxed); }

    void setParameter(uint32_t id, float value) noexcept override {
        if (id == 0 && std::isfinite(value)) {
            setThreshold(-24.0f + std::clamp(value, 0.0f, 1.0f) * 24.0f);
        }
    }
    float getParameter(uint32_t id) const noexcept override {
        const float threshold = m_threshold.load(std::memory_order_relaxed);
        if (id != 0 || threshold <= 0.0f) return 0.0f;
        const float db = 20.0f * std::log10(threshold);
        return std::clamp((db + 24.0f) / 24.0f, 0.0f, 1.0f);
    }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        const char* name = id == 0 ? "Threshold" : "";
        std::strncpy(outName, name, maxSize - 1);
        outName[maxSize - 1] = '\0';
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(sizeof(float));
        const float threshold = m_threshold.load(std::memory_order_relaxed);
        std::memcpy(state.data(), &threshold, sizeof(float));
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != sizeof(float)) return false;
        float threshold = 0.0f;
        std::memcpy(&threshold, state.data(), sizeof(float));
        if (!std::isfinite(threshold) || threshold <= 0.0f || threshold > 1.0f) return false;
        m_threshold.store(threshold, std::memory_order_relaxed);
        return true;
    }
    bool restoreStateChecked(const std::vector<uint8_t>& state) override {
        if (state.size() != sizeof(float)) return false;
        float threshold = 0.0f;
        std::memcpy(&threshold, state.data(), sizeof(float));
        if (!std::isfinite(threshold) || threshold <= 0.0f || threshold > 1.0f) return false;
        m_threshold.store(threshold, std::memory_order_relaxed);
        return true;
    }

private:
    double m_sampleRate;
    std::atomic<float> m_threshold{1.0f};
    float m_envelope = 1.0f;
    float m_release = 0.999f;
    std::array<float, 1024> m_delayL, m_delayR;
    uint32_t m_writeIdx = 0;
};

} // namespace Aura::DSP::Effects
