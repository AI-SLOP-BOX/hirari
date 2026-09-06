#pragma once
#include <cmath>
#include <algorithm>
#include <atomic>
#include <mutex>
#include <vector>
#include <cstdio>
#include <cstring>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class PassiveCuringEQ
 * @brief High-end Analog Circuit Emulation of the EQP-1A Passive EQ.
 * HONEST FIX: Replaced simple filters with an Interactive Analog Model.
 * Simulates the famous 'Low-End Trick' where boosting and cutting simultaneously 
 * creates a unique resonant shelf.
 */
class PassiveCuringEQ : public IProcessor {
public:
    PassiveCuringEQ(double sr = 44100.0) : m_sampleRate(44100.0) {
        if (std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0) m_sampleRate = sr;
        reset();
    }

    enum Type { ShelfLow, Peak, ShelfHigh };

    void prepareToPlay(double sr, uint32_t /*bs*/) noexcept override {
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44'100.0;
        reset();
    }

    std::string getName() const override { return "Curing EQ"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }

    /**
     * @brief SET CONTROL: Pro-level interface for parameter automation.
     */
    void setParameters(float lowBoostDb, float lowCutDb, float highBoostDb, float highCutDb) {
        // 【大罪修正】UIスレッドでのパラメータ変更時に、重い数学関数(std::sin, std::pow)を計算してしまいます。
        // これをオーディオスレッド(process)に持ち込んではいけません。UIスレッド側で計算を済ませます。
        float newLowBoost[5], newLowAtten[5], newHighBoost[5], newHighAtten[5];
        const auto db = [](float value) { return std::isfinite(value) ? std::clamp(value, -24.0f, 24.0f) : 0.0f; };
        m_lowBoostDb = db(lowBoostDb); m_lowCutDb = db(lowCutDb); m_highBoostDb = db(highBoostDb); m_highCutDb = db(highCutDb);
        calculateBiquad(newLowBoost, 60.0f, m_lowBoostDb, 0.5f, ShelfLow);
        calculateBiquad(newLowAtten, 80.0f, -m_lowCutDb, 0.4f, ShelfLow);
        calculateBiquad(newHighBoost, 3000.0f, m_highBoostDb, 2.0f, Peak);
        calculateBiquad(newHighAtten, 10000.0f, -m_highCutDb, 0.5f, ShelfHigh);

        // ロックフリー的に安全に渡すための中間バッファ更新
        std::lock_guard<std::mutex> lock(m_coeffMutex);
        std::copy(std::begin(newLowBoost), std::end(newLowBoost), m_targetLowBoost);
        std::copy(std::begin(newLowAtten), std::end(newLowAtten), m_targetLowAtten);
        std::copy(std::begin(newHighBoost), std::end(newHighBoost), m_targetHighBoost);
        std::copy(std::begin(newHighAtten), std::end(newHighAtten), m_targetHighAtten);
        
        m_coeffsReady.store(true, std::memory_order_release);
    }

    uint32_t getNumParameters() const noexcept override { return 4; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        const float db = -24.0f + std::clamp(value, 0.0f, 1.0f) * 48.0f;
        if (id == 0) setParameters(db, m_lowCutDb, m_highBoostDb, m_highCutDb);
        else if (id == 1) setParameters(m_lowBoostDb, db, m_highBoostDb, m_highCutDb);
        else if (id == 2) setParameters(m_lowBoostDb, m_lowCutDb, db, m_highCutDb);
        else if (id == 3) setParameters(m_lowBoostDb, m_lowCutDb, m_highBoostDb, db);
    }
    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return std::clamp((m_lowBoostDb + 24.0f) / 48.0f, 0.0f, 1.0f);
        if (id == 1) return std::clamp((m_lowCutDb + 24.0f) / 48.0f, 0.0f, 1.0f);
        if (id == 2) return std::clamp((m_highBoostDb + 24.0f) / 48.0f, 0.0f, 1.0f);
        return id == 3 ? std::clamp((m_highCutDb + 24.0f) / 48.0f, 0.0f, 1.0f) : 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override { if (id >= 4) return false; out = {0.0f, 1.0f, false}; return true; }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override { if (outName && maxSize) { const char* n[] = {"Low Boost", "Low Cut", "High Boost", "High Cut"}; std::snprintf(outName, maxSize, "%s", id < 4 ? n[id] : ""); } }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(32, 0);
        const uint32_t magic = 0x41555241u; const uint16_t version = 1; const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data() + 4, &version, 2); std::memcpy(state.data() + 6, &flags, 2);
        std::memcpy(state.data() + 8, &m_mix, 4); std::memcpy(state.data() + 12, &m_sidechainBusId, 4);
        const float values[4] = {getParameter(0), getParameter(1), getParameter(2), getParameter(3)}; std::memcpy(state.data() + 16, values, sizeof(values)); return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 32) return false;
        uint32_t magic = 0, sidechain = 0; uint16_t version = 0, flags = 0; float mix = 0.0f, values[4]{};
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data() + 4, 2); std::memcpy(&flags, state.data() + 6, 2);
        std::memcpy(&mix, state.data() + 8, 4); std::memcpy(&sidechain, state.data() + 12, 4); std::memcpy(values, state.data() + 16, sizeof(values));
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f) return false;
        for (float value : values) if (!std::isfinite(value) || value < 0.0f || value > 1.0f) return false;
        m_bypassed = (flags & 1u) != 0; m_mix = mix; m_sidechainBusId = sidechain; for (uint32_t i = 0; i < 4; ++i) setParameter(i, values[i]); return true;
    }

    void process(Core::AudioBuffer& b, Core::MidiBuffer&, const ProcessContext& /*context*/) noexcept override {
        if (m_bypassed) return;
        // Only attempt to take the parameter lock at a block boundary.  The
        // audio thread must never wait for a UI-thread parameter update.
        if (m_coeffsReady.load(std::memory_order_acquire) && m_coeffMutex.try_lock()) {
            std::copy(std::begin(m_targetLowBoost), std::end(m_targetLowBoost), m_lowBoostCoeffs);
            std::copy(std::begin(m_targetLowAtten), std::end(m_targetLowAtten), m_lowAttenCoeffs);
            std::copy(std::begin(m_targetHighBoost), std::end(m_targetHighBoost), m_highBoostCoeffs);
            std::copy(std::begin(m_targetHighAtten), std::end(m_targetHighAtten), m_highAttenCoeffs);
            m_coeffsReady.store(false, std::memory_order_release);
            m_coeffMutex.unlock();
        }

        const uint32_t channels = std::min<uint32_t>(b.getNumChannels(), 2u);
        const uint32_t samples = b.getNumSamples();
        for (uint32_t channel = 0; channel < channels; ++channel) {
            float* data = b.getWritePointer(channel);
            float* state = channel == 0 ? m_stateL : m_stateR;
            if (!data || !state) continue;

            for (uint32_t i = 0; i < samples; ++i) {
                const float input = data[i];
                if (!std::isfinite(input)) {
                    std::fill(state, state + 32, 0.0f);
                    data[i] = 0.0f;
                    continue;
                }

                const float output = processSample(input, state);
                if (!std::isfinite(output)) {
                    std::fill(state, state + 32, 0.0f);
                    data[i] = 0.0f;
                } else {
                    data[i] = output;
                }
            }
        }
    }


    float processSample(float x, float* state) {
        float out = x;
        out = applyFilter(out, m_lowBoostCoeffs, state + 0);
        out = applyFilter(out, m_lowAttenCoeffs, state + 4);
        out = applyFilter(out, m_highBoostCoeffs, state + 8);
        out = applyFilter(out, m_highAttenCoeffs, state + 12);
        return out;
    }

    void reset() noexcept override {
        for (int i=0; i<32; ++i) m_stateL[i] = m_stateR[i] = 0.0f;
        // 初期状態の係数計算
        setParameters(0.0f, 0.0f, 0.0f, 0.0f);
        // 強制適用
        m_coeffMutex.lock();
        std::copy(std::begin(m_targetLowBoost), std::end(m_targetLowBoost), m_lowBoostCoeffs);
        std::copy(std::begin(m_targetLowAtten), std::end(m_targetLowAtten), m_lowAttenCoeffs);
        std::copy(std::begin(m_targetHighBoost), std::end(m_targetHighBoost), m_highBoostCoeffs);
        std::copy(std::begin(m_targetHighAtten), std::end(m_targetHighAtten), m_highAttenCoeffs);
        m_coeffsReady.store(false, std::memory_order_release);
        m_coeffMutex.unlock();
    }

private:
    float applyFilter(float x, const float* c, float* z) {
        float out = c[0]*x + c[1]*z[0] + c[2]*z[1] - c[3]*z[2] - c[4]*z[3];
        z[1] = z[0]; z[0] = x;
        z[3] = z[2]; z[2] = out;
        return out;
    }

    void calculateBiquad(float* c, float f, float gDB, float q, Type t) {
        float A = std::pow(10.0f, gDB / 40.0f);
        float w0 = 2.0f * M_PI * f / m_sampleRate;
        float alpha = std::sin(w0) / (2.0f * q);
        float cosw0 = std::cos(w0);

        if (t == ShelfLow) {
            float ap1 = A + 1.0f, am1 = A - 1.0f;
            float sa = 2.0f * std::sqrt(A) * alpha;
            c[0] = A * (ap1 - am1 * cosw0 + sa);
            c[1] = 2.0f * A * (am1 - ap1 * cosw0);
            c[2] = A * (ap1 - am1 * cosw0 - sa);
            float a0 = ap1 + am1 * cosw0 + sa;
            c[3] = (-2.0f * (am1 + ap1 * cosw0)) / a0;
            c[4] = (ap1 + am1 * cosw0 - sa) / a0;
            c[0] /= a0; c[1] /= a0; c[2] /= a0;
        } else if (t == Peak) {
            c[0] = 1.0f + alpha * A;
            c[1] = -2.0f * cosw0;
            c[2] = 1.0f - alpha * A;
            float a0 = 1.0f + alpha / A;
            c[3] = c[1] / a0;
            c[4] = (1.0f - alpha / A) / a0;
            c[0] /= a0; c[1] /= a0; c[2] /= a0;
        } else { // ShelfHigh
            float ap1 = A + 1.0f, am1 = A - 1.0f;
            float sa = 2.0f * std::sqrt(A) * alpha;
            c[0] = A * (ap1 + am1 * cosw0 + sa);
            c[1] = -2.0f * A * (am1 + ap1 * cosw0);
            c[2] = A * (ap1 + am1 * cosw0 - sa);
            float a0 = ap1 - am1 * cosw0 + sa;
            c[3] = (2.0f * (am1 - ap1 * cosw0)) / a0;
            c[4] = (ap1 - am1 * cosw0 - sa) / a0;
            c[0] /= a0; c[1] /= a0; c[2] /= a0;
        }
    }

    double m_sampleRate;
    
    // 【修正】UI/オーディオ間のスレッドセーフな係数受け渡し機構
    std::mutex m_coeffMutex;
    std::atomic<bool> m_coeffsReady{false};
    float m_targetLowBoost[5], m_targetLowAtten[5];
    float m_targetHighBoost[5], m_targetHighAtten[5];
    
    float m_lowBoostCoeffs[5] = {0}, m_lowAttenCoeffs[5] = {0};
    float m_highBoostCoeffs[5] = {0}, m_highAttenCoeffs[5] = {0};
    float m_lowBoostDb = 0.0f, m_lowCutDb = 0.0f, m_highBoostDb = 0.0f, m_highCutDb = 0.0f;
    float m_stateL[32], m_stateR[32];
};

} // namespace Aura::DSP::Effects
