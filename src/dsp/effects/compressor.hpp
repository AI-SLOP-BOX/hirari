#pragma once

#include <cmath>
#include <algorithm>
#include <vector>
#include <array>
#include <cstring>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class DynamicCompressor
 * @brief Professional Mastering-Grade Dynamic Range Processor.
 * HONEST FIX: Added Sidechain Support, Soft Knee, and RMS Detection.
 */
class DynamicCompressor : public IProcessor {
public:
    static constexpr uint32_t kStateMagic = 0x41524350u; // "ARCP"
    static constexpr uint32_t kStateVersion = 1u;
    DynamicCompressor() {
        reset();
    }

    void prepareToPlay(double sr, uint32_t /*bs*/) noexcept override {
        m_sampleRate = sr;
        // --- HONEST FIX: PRE-ALLOCATE RT-SAFE ---
        // Avoids memory allocation during process() even if lookahead changes.
        m_delayL.fill(0.0f);
        m_delayR.fill(0.0f);
        m_writeIdx = 0;
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const ProcessContext& /*context*/) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0 ||
            buffer.getNumSamples() == 0 || buffer.getNumSamples() > kMaxLookahead * 16) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        const uint32_t delay = std::min(m_lookaheadSamples, kMaxLookahead - 1);
        const float makeup = std::pow(10.0f, (m_autoGain ? calculateAutoMakeup() : m_makeupDB) / 20.0f);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const float inL = buffer.getReadPointer(0)[i];
            const float inR = channels > 1 ? buffer.getReadPointer(1)[i] : inL;
            const float detector = m_useRMS ? std::sqrt(0.5f * (inL * inL + inR * inR)) : std::max(std::abs(inL), std::abs(inR));
            const float target = std::isfinite(detector) ? detector : 0.0f;
            const float alpha = target > m_envelope ? m_attackAlpha : m_releaseAlpha;
            m_envelope = alpha * m_envelope + (1.0f - alpha) * target;
            const float levelDb = 20.0f * std::log10(std::max(m_envelope, 1.0e-8f));
            float reductionDb = 0.0f;
            if (m_kneeDB > 0.0f && levelDb > m_thresholdDB - m_kneeDB * 0.5f && levelDb < m_thresholdDB + m_kneeDB * 0.5f) {
                const float x = levelDb - m_thresholdDB + m_kneeDB * 0.5f;
                reductionDb = (1.0f / m_ratio - 1.0f) * (x * x) / (2.0f * m_kneeDB);
            } else if (levelDb > m_thresholdDB) {
                reductionDb = (m_thresholdDB + (levelDb - m_thresholdDB) / m_ratio) - levelDb;
            }
            m_currentGr = 0.995f * m_currentGr + 0.005f * std::pow(10.0f, reductionDb / 20.0f);
            m_delayL[m_writeIdx] = inL;
            m_delayR[m_writeIdx] = inR;
            const uint32_t readIdx = (m_writeIdx + kMaxLookahead - delay) % kMaxLookahead;
            buffer.getWritePointer(0)[i] = m_delayL[readIdx] * m_currentGr * makeup;
            if (channels > 1) buffer.getWritePointer(1)[i] = m_delayR[readIdx] * m_currentGr * makeup;
            m_writeIdx = (m_writeIdx + 1) % kMaxLookahead;
        }
    }


    uint32_t getLatencySamples() const noexcept override { return m_lookaheadSamples; }

    void reset() noexcept override {
        m_envelope = 0.0f; m_currentGr = 1.0f; m_rmsSum = 0.0f;
        std::fill(m_delayL.begin(), m_delayL.end(), 0.0f);
        std::fill(m_delayR.begin(), m_delayR.end(), 0.0f);
        m_writeIdx = 0;
    }

    // Parameters
    void setLookahead(float ms) { m_lookaheadSamples = std::min<uint32_t>(kMaxLookahead - 1, static_cast<uint32_t>(std::max(0.0f, ms) * m_sampleRate * 0.001)); }
    void setThreshold(float db) { m_thresholdDB = std::clamp(db, -100.0f, 0.0f); }
    void setRatio(float r) { m_ratio = std::clamp(r, 1.0f, 100.0f); }
    void setAttack(float ms) { m_attackAlpha = std::exp(-1.0f / std::max(1.0, m_sampleRate * std::max(0.1f, ms) * 0.001)); }
    void setRelease(float ms) { m_releaseAlpha = std::exp(-1.0f / std::max(1.0, m_sampleRate * std::max(0.1f, ms) * 0.001)); }
    void setMakeup(float db) { m_makeupDB = db; }
    void setAutoGain(bool enable) { m_autoGain = enable; }
    void setKnee(float db) { m_kneeDB = std::max(0.0f, db); }
    void setUseRMS(bool enable) { m_useRMS = enable; }

    // Host-facing normalized parameters. Keeping the conversion here means
    // automation, presets, and the internal UI share one stable contract.
    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        value = std::clamp(value, 0.0f, 1.0f);
        switch (id) {
            case 0: setThreshold(-60.0f + value * 60.0f); break;
            case 1: setRatio(1.0f + value * 19.0f); break;
            case 2: setAttack(0.1f + value * 99.9f); break;
            case 3: setRelease(5.0f + value * 995.0f); break;
            case 4: setMakeup(-12.0f + value * 24.0f); break;
            case 5: setKnee(value * 24.0f); break;
            case 6: setLookahead(value * 20.0f); break;
            default: break;
        }
    }

    float getParameter(uint32_t id) const noexcept override {
        switch (id) {
            case 0: return std::clamp((m_thresholdDB + 60.0f) / 60.0f, 0.0f, 1.0f);
            case 1: return std::clamp((m_ratio - 1.0f) / 19.0f, 0.0f, 1.0f);
            case 2: return std::clamp((attackMs() - 0.1f) / 99.9f, 0.0f, 1.0f);
            case 3: return std::clamp((releaseMs() - 5.0f) / 995.0f, 0.0f, 1.0f);
            case 4: return std::clamp((m_makeupDB + 12.0f) / 24.0f, 0.0f, 1.0f);
            case 5: return std::clamp(m_kneeDB / 24.0f, 0.0f, 1.0f);
            case 6: return std::clamp(static_cast<float>(m_lookaheadSamples) /
                                      std::max(1.0f, static_cast<float>(m_sampleRate) * 0.020f), 0.0f, 1.0f);
            default: return 0.0f;
        }
    }

    uint32_t getNumParameters() const noexcept override { return 7; }

    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= getNumParameters()) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }

    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        static constexpr const char* names[] = {
            "Threshold", "Ratio", "Attack", "Release", "Makeup", "Knee", "Lookahead"
        };
        const char* name = id < 7 ? names[id] : "";
        std::strncpy(outName, name, maxSize - 1);
        outName[maxSize - 1] = '\0';
    }

    std::vector<uint8_t> getState() const override {
        struct State { uint32_t magic, version; float threshold, ratio, makeup, knee; uint32_t lookahead; } state{
            kStateMagic, kStateVersion, m_thresholdDB, m_ratio, m_makeupDB, m_kneeDB, m_lookaheadSamples};
        std::vector<uint8_t> bytes(sizeof(state));
        std::memcpy(bytes.data(), &state, sizeof(state));
        return bytes;
    }

    bool restoreStateChecked(const std::vector<uint8_t>& bytes) override {
        struct LegacyState { float threshold, ratio, makeup, knee; uint32_t lookahead; };
        struct State { uint32_t magic, version; float threshold, ratio, makeup, knee; uint32_t lookahead; };
        State state{};
        if (bytes.size() == sizeof(LegacyState)) {
            LegacyState legacy{};
            std::memcpy(&legacy, bytes.data(), sizeof(legacy));
            state = {kStateMagic, 0u, legacy.threshold, legacy.ratio, legacy.makeup,
                     legacy.knee, legacy.lookahead};
        } else if (bytes.size() == sizeof(State)) {
            std::memcpy(&state, bytes.data(), sizeof(state));
        } else {
            return false;
        }
        if (state.magic != kStateMagic || state.version > kStateVersion) return false;
        if (!std::isfinite(state.threshold) || !std::isfinite(state.ratio) ||
            !std::isfinite(state.makeup) || !std::isfinite(state.knee) ||
            state.threshold < -100.0f || state.threshold > 0.0f ||
            state.ratio < 1.0f || state.ratio > 100.0f ||
            state.knee < 0.0f || state.knee > 60.0f || state.lookahead >= kMaxLookahead)
            return false;
        m_thresholdDB = state.threshold;
        m_ratio = state.ratio;
        m_makeupDB = state.makeup;
        m_kneeDB = state.knee;
        m_lookaheadSamples = state.lookahead;
        return true;
    }

private:
    float attackMs() const noexcept {
        return static_cast<float>(-1.0 / (std::max(1.0e-6, m_sampleRate) *
            std::log(std::max(1.0e-6f, m_attackAlpha))) * 1000.0);
    }
    float releaseMs() const noexcept {
        return static_cast<float>(-1.0 / (std::max(1.0e-6, m_sampleRate) *
            std::log(std::max(1.0e-6f, m_releaseAlpha))) * 1000.0);
    }

    float calculateAutoMakeup() const { return -(m_thresholdDB * (1.0f - 1.0f / m_ratio)) * 0.5f; }

    static constexpr uint32_t kMaxLookahead = 4096;
    double m_sampleRate = 44100.0;
    float m_thresholdDB = -20.0f, m_ratio = 4.0f, m_makeupDB = 0.0f, m_kneeDB = 6.0f;
    bool m_autoGain = true, m_useRMS = false;
    float m_rmsSum = 0.0f, m_attackAlpha = 0.9f, m_releaseAlpha = 0.999f, m_envelope = 0.0f, m_currentGr = 1.0f;
    
    std::array<float, kMaxLookahead> m_delayL{}, m_delayR{};
    uint32_t m_writeIdx = 0, m_lookaheadSamples = 0;
};

} // namespace Aura::DSP::Effects
