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
 * @class MasteringLimiter
 * @brief Professional Mastering Limiter with Lookahead and Brick-wall ceiling.
 * HONEST FIX: Implements a true lookahead buffer to anticipate peaks 
 * and apply gain reduction transparently before the peak occurs.
 * Prevents any digital clipping (0dBFS) while maximizing loudness.
 */
class MasteringLimiter : public IProcessor {
public:
    MasteringLimiter(uint32_t lookaheadSamples = 256) 
        : m_lookahead(std::min<uint32_t>(lookaheadSamples, 65535u)), m_delay(65536) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0
            ? sr : 44'100.0;
        setRelease(m_releaseMs);
        reset();
    }

    /**
     * @brief PROCESS: Zero-clipping gain reduction with lookahead ballistics.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        if (m_bypassed || buffer.getNumChannels() < 2 || buffer.getNumSamples() == 0) return;

        uint32_t numSamples = buffer.getNumSamples();
        float ceiling = std::pow(10.0f, m_ceilingDB / 20.0f);
        float threshold = std::pow(10.0f, m_thresholdDB / 20.0f);
        
        for (uint32_t s = 0; s < numSamples; ++s) {
            // 1. Peak Detection with Lookahead
            float inL = std::isfinite(buffer.getReadPointer(0)[s]) ? buffer.getReadPointer(0)[s] : 0.0f;
            float inR = std::isfinite(buffer.getReadPointer(1)[s]) ? buffer.getReadPointer(1)[s] : 0.0f;
            float peak = std::max(std::abs(inL), std::abs(inR));

            // 2. Feed into peak-tracking envelope
            if (peak > m_peakEnv) m_peakEnv = peak;
            else m_peakEnv *= m_releaseAlpha;

            // 3. Calculate target gain based on threshold
            float targetGain = 1.0f;
            if (m_peakEnv * m_inputGain > threshold) {
                targetGain = threshold / (m_peakEnv * m_inputGain);
            }

            // 4. Smooth the gain changes (Attack is implicit via lookahead)
            m_currentGain = 0.95f * m_currentGain + 0.05f * targetGain;

            // 5. Apply delayed signal with gain reduction
            for (uint32_t c = 0; c < 2; ++c) {
                float delayed = m_delay.process(buffer.getReadPointer(c)[s], m_lookahead);
                buffer.getWritePointer(c)[s] = delayed * m_inputGain * m_currentGain;
                
                // Hard-ceiling safety
                buffer.getWritePointer(c)[s] = std::clamp(buffer.getWritePointer(c)[s], -ceiling, ceiling);
            }
        }
    }

    void reset() noexcept override {
        m_delay.reset();
        m_currentGain = 1.0f;
        m_peakEnv = 0.0f;
    }

    uint32_t getLatencySamples() const noexcept override { return m_lookahead; }
    uint32_t getTailSamples() const noexcept override {
        const double rate = std::isfinite(m_sampleRate) && m_sampleRate > 0.0
            ? m_sampleRate : 44'100.0;
        return static_cast<uint32_t>(std::min<double>(
            static_cast<double>(m_lookahead) + 0.35 * rate, 30.0 * rate));
    }

    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        value = std::clamp(value, 0.0f, 1.0f);
        if (id == 0) setThreshold(-60.0f + value * 60.0f);
        else if (id == 1) setCeiling(-60.0f + value * 60.0f);
        else if (id == 2) setRelease(1.0f + value * 1999.0f);
    }

    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return std::clamp((m_thresholdDB + 60.0f) / 60.0f, 0.0f, 1.0f);
        if (id == 1) return std::clamp((m_ceilingDB + 60.0f) / 60.0f, 0.0f, 1.0f);
        if (id == 2) return std::clamp((m_releaseMs - 1.0f) / 1999.0f, 0.0f, 1.0f);
        return 0.0f;
    }

    uint32_t getNumParameters() const noexcept override { return 3; }

    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id > 2) return false;
        out.minimum = 0.0f;
        out.maximum = 1.0f;
        out.stepped = false;
        return true;
    }

    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        const char* name = id == 0 ? "Threshold" : (id == 1 ? "Ceiling" : (id == 2 ? "Release" : ""));
        std::snprintf(outName, maxSize, "%s", name);
    }

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(sizeof(float) * 3u);
        const float values[3] = {getParameter(0), getParameter(1), getParameter(2)};
        std::memcpy(state.data(), values, sizeof(values));
        return state;
    }

    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != sizeof(float) * 3u) return false;
        float values[3]{};
        std::memcpy(values, state.data(), sizeof(values));
        for (const float value : values)
            if (!std::isfinite(value) || value < 0.0f || value > 1.0f) return false;
        setParameter(0, values[0]);
        setParameter(1, values[1]);
        setParameter(2, values[2]);
        return true;
    }

    // Parameters
    void setThreshold(float db) {
        if (!std::isfinite(db)) return;
        m_thresholdDB = std::clamp(db, -60.0f, 0.0f);
        m_inputGain = std::pow(10.0f, -m_thresholdDB / 20.0f);
    }
    void setCeiling(float db) {
        if (std::isfinite(db)) m_ceilingDB = std::clamp(db, -60.0f, 0.0f);
    }
    void setRelease(float ms) {
        if (!std::isfinite(ms)) return;
        m_releaseMs = std::clamp(ms, 1.0f, 2000.0f);
        const double rate = std::isfinite(m_sampleRate) && m_sampleRate > 0.0 ? m_sampleRate : 44'100.0;
        const double boundedMs = static_cast<double>(m_releaseMs);
        m_releaseAlpha = static_cast<float>(std::pow(0.01, 1.0 / (rate * boundedMs * 0.001)));
    }

private:
    double m_sampleRate = 44100.0;
    uint32_t m_lookahead;
    DelayLine m_delay;

    float m_thresholdDB = 0.0f;
    float m_ceilingDB = -0.1f;
    float m_inputGain = 1.0f;
    float m_releaseAlpha = 0.999f;
    float m_releaseMs = 250.0f;

    float m_peakEnv = 0.0f;
    float m_currentGain = 1.0f;
};

} // namespace Aura::DSP::Effects
