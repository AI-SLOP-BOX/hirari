#pragma once
#include <vector>
#include <algorithm>
#include <cmath>
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

    void prepareToPlay(double sr, uint32_t /*bs*/) noexcept override {
        m_sampleRate = sr;
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
        const float releaseMs = std::isfinite(m_releaseMs) ? std::clamp(m_releaseMs, 1.0f, 1000.0f) : 50.0f;

        float releaseCoef = std::exp(-1.0f / (releaseMs * 0.001f * static_cast<float>(sr)));
        if (!std::isfinite(releaseCoef)) releaseCoef = 0.99f;
        releaseCoef = std::clamp(releaseCoef, 0.0f, 0.9999f);

        uint32_t lookaheadSamples = static_cast<uint32_t>(std::round(m_lookaheadMs * 0.001f * sr));
        lookaheadSamples = std::clamp<uint32_t>(lookaheadSamples, 0, kMaxLookahead - 1);

        for (uint32_t s = 0; s < numSamples; ++s) {
            float inL = std::isfinite(l[s]) ? l[s] * m_thresholdGain : 0.0f;
            float inR = r && std::isfinite(r[s]) ? r[s] * m_thresholdGain : inL;

            float peak = std::max(std::abs(inL), std::abs(inR));

            m_delayL[m_writeIdx] = inL;
            m_delayR[m_writeIdx] = inR;

            uint32_t readIdx = (m_writeIdx + kMaxLookahead - lookaheadSamples) & (kMaxLookahead - 1);
            float outL = m_delayL[readIdx];
            float outR = m_delayR[readIdx];
            m_writeIdx = (m_writeIdx + 1) & (kMaxLookahead - 1);

            float targetAtten = (peak > m_ceiling) ? m_ceiling / (peak + 1e-6f) : 1.0f;
            if (!std::isfinite(targetAtten)) targetAtten = 1.0f;

            if (targetAtten < m_currentGain) {
                m_currentGain = targetAtten; // Instant attack for limiting
            } else {
                m_currentGain = m_currentGain * releaseCoef + targetAtten * (1.0f - releaseCoef);
            }
            if (!std::isfinite(m_currentGain)) m_currentGain = 1.0f;

            l[s] = outL * m_currentGain;
            if (r) r[s] = outR * m_currentGain;
        }
    }

    void setThreshold(float db) { 
        if (std::isfinite(db)) {
            m_thresholdGain = std::pow(10.0f, std::clamp(db, -60.0f, 24.0f) / 20.0f); 
        }
    }
    void setCeiling(float db) { 
        if (std::isfinite(db)) {
            m_ceiling = std::pow(10.0f, std::clamp(db, -60.0f, 0.0f) / 20.0f); 
        }
    }
    void setRelease(float ms) {
        if (std::isfinite(ms)) {
            m_releaseMs = std::clamp(ms, 1.0f, 1000.0f);
        }
    }
    void setLookaheadMs(float ms) {
        if (std::isfinite(ms)) {
            m_lookaheadMs = std::clamp(ms, 0.0f, 20.0f);
        }
    }

    void reset() noexcept override {
        m_currentGain = 1.0f;
        std::fill(m_delayL.begin(), m_delayL.end(), 0.0f);
        std::fill(m_delayR.begin(), m_delayR.end(), 0.0f);
        m_writeIdx = 0;
    }

private:
    double m_sampleRate;
    float m_ceiling = 0.99f;
    float m_thresholdGain = 1.0f;
    float m_currentGain = 1.0f;
    float m_releaseMs = 50.0f;
    float m_lookaheadMs = 2.0f;
    
    std::vector<float> m_delayL;
    std::vector<float> m_delayR;
    uint32_t m_writeIdx = 0;
};

} // namespace Aura::DSP::Effects
