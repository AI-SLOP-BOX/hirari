#pragma once
#include <vector>
#include <cstdio>
#include <cstring>
#include <cmath>
#include <algorithm>
#include <array>
#include <limits>
#include "../iprocessor.hpp"
#include "../utils/dsp_utils.hpp"

namespace Aura::DSP::Effects {

/**
 * @class GranularCloud
 * @brief Professional Granular Synthesis for Ambient and Ethereal textures.
 */
class GranularCloud : public IProcessor {
public:
    static constexpr size_t kMaxGrains = 16;
    static constexpr size_t kWindowSize = 1024;

    struct Grain {
        double currentPos = 0.0;
        uint32_t length = 0;
        float pitch = 1.0f;
        float env = 0.0f;
        bool active = false;
    };

    GranularCloud() : m_writeIdx(0) {
        setMix(0.35f);
        // Pre-initialize window table (Hanning)
        for (size_t i = 0; i < kWindowSize; ++i) {
            m_windowTable[i] = 0.5f * (1.0f - std::cos(Utils::DSPUtils::TWO_PI * i / kWindowSize));
        }
        m_grains.resize(kMaxGrains);
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        if (!std::isfinite(sr) || sr < 1.0 || sr > 384000.0) return;

        // Pre-allocate buffer for 2 seconds (No resize in audio thread)
        const double requestedSamples = sr * 2.0;
        if (!std::isfinite(requestedSamples)
            || requestedSamples < 1.0
            || requestedSamples > static_cast<double>(std::numeric_limits<uint32_t>::max())) {
            return;
        }
        const uint32_t samples = static_cast<uint32_t>(requestedSamples);
        m_sampleRate = sr;
        if (m_circBufferL.size() < samples) {
            m_circBufferL.assign(samples, 0.0f);
            m_circBufferR.assign(samples, 0.0f);
        }
    }

    // A spawned grain can remain audible for up to 10,000 samples.  Report
    // that state so bounce/export does not truncate ambient tails.
    uint32_t getTailSamples() const noexcept override { return 10000u; }
    std::string getName() const override { return "Granular Cloud"; }
    uint32_t getNumParameters() const noexcept override { return 2; }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_mix : (id == 1 ? m_density : 0.0f); }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override { if (id >= 2) return false; out = {0.0f, 1.0f, false}; return true; }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override { if (outName && maxSize) std::snprintf(outName, maxSize, "%s", id == 0 ? "Mix" : (id == 1 ? "Density" : "")); }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(24, 0);
        const uint32_t magic = 0x41555241u; const uint16_t version = 1; const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data() + 4, &version, 2); std::memcpy(state.data() + 6, &flags, 2);
        std::memcpy(state.data() + 8, &m_mix, 4); std::memcpy(state.data() + 12, &m_sidechainBusId, 4);
        const float values[2] = {m_mix, m_density}; std::memcpy(state.data() + 16, values, sizeof(values));
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 24) return false;
        uint32_t magic = 0, sidechain = 0; uint16_t version = 0, flags = 0; float mix = 0.0f, values[2]{};
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data() + 4, 2); std::memcpy(&flags, state.data() + 6, 2);
        std::memcpy(&mix, state.data() + 8, 4); std::memcpy(&sidechain, state.data() + 12, 4); std::memcpy(values, state.data() + 16, sizeof(values));
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f ||
            !std::isfinite(values[0]) || !std::isfinite(values[1]) || values[0] < 0.0f || values[0] > 1.0f || values[1] < 0.0f || values[1] > 1.0f) return false;
        m_bypassed = (flags & 1u) != 0; m_sidechainBusId = sidechain; setMix(values[0]); setParameter(1, values[1]);
        return true;
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        if (m_bypassed || buffer.isEmpty() || m_circBufferL.empty()
            || m_circBufferL.size() != m_circBufferR.size()) {
            return;
        }
        const uint32_t channels = buffer.getNumChannels();
        float* left = buffer.getWritePointer(0);
        float* right = channels > 1 ? buffer.getWritePointer(1) : left;
        if (!left || !right) return;
        const size_t capacity = m_circBufferL.size();
        const float mix = std::clamp(std::isfinite(m_mix) ? m_mix : 0.0f, 0.0f, 1.0f);
        const float density = std::clamp(std::isfinite(m_density) ? m_density : 0.0f, 0.0f, 1.0f);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const float dryL = std::isfinite(left[i]) ? left[i] : 0.0f;
            const float dryR = std::isfinite(right[i]) ? right[i] : dryL;
            m_circBufferL[m_writeIdx] = dryL;
            m_circBufferR[m_writeIdx] = dryR;
            float wetL = 0.0f;
            float wetR = 0.0f;
            if (density > 0.0f && (fastRand() % 10000u) < static_cast<uint32_t>(density * 100.0f)) spawnGrain();
            for (size_t gidx = 0; gidx < m_grains.size(); ++gidx) {
                Grain& grain = m_grains[gidx];
                if (!grain.active || grain.length == 0) continue;
                const uint32_t age = m_grainAge[gidx];
                if (age >= grain.length) { grain.active = false; continue; }
                const size_t read = static_cast<size_t>(grain.currentPos) % capacity;
                const size_t windowIndex = std::min<size_t>(age, kWindowSize - 1);
                const float env = m_windowTable[windowIndex];
                wetL += m_circBufferL[read] * env * 0.08f;
                wetR += m_circBufferR[read] * env * 0.08f;
                grain.currentPos += grain.pitch;
                if (grain.currentPos >= static_cast<double>(capacity)) grain.currentPos -= static_cast<double>(capacity);
                m_grainAge[gidx] = age + 1;
            }
            left[i] = std::isfinite(dryL + mix * (wetL - dryL)) ? dryL + mix * (wetL - dryL) : 0.0f;
            right[i] = std::isfinite(dryR + mix * (wetR - dryR)) ? dryR + mix * (wetR - dryR) : 0.0f;
            m_writeIdx = (m_writeIdx + 1u) % static_cast<uint32_t>(capacity);
        }
    }


    void setParameter(uint32_t id, float value) noexcept override {
        if (id == 0) m_mix = std::isfinite(value) ? std::clamp(value, 0.0f, 1.0f) : 0.0f;
        else if (id == 1) m_density = std::isfinite(value) ? std::clamp(value, 0.0f, 1.0f) : 0.0f;
    }

    void reset() noexcept override {
        std::fill(m_circBufferL.begin(), m_circBufferL.end(), 0.0f);
        std::fill(m_circBufferR.begin(), m_circBufferR.end(), 0.0f);
        for (auto& g : m_grains) g.active = false;
        m_grainAge.fill(0);
        m_writeIdx = 0;
        m_spawnAccumulator = 0.0f;
    }

private:
    void spawnGrain() {
        if (m_circBufferL.empty() || m_circBufferL.size() != m_circBufferR.size()) return;

        for (uint32_t i = 0; i < kMaxGrains; ++i) {
            auto& g = m_grains[i];
            if (!g.active) {
                g.active = true;
                const size_t bufSize = m_circBufferL.size();
                uint32_t offset = fastRand() % static_cast<uint32_t>(std::min<size_t>(44100, bufSize)); // Up to 1sec ago
                g.currentPos = (static_cast<size_t>(m_writeIdx) + bufSize - offset % bufSize) % bufSize;
                g.length = 2000 + (fastRand() % 8000);
                g.pitch = 0.5f + (fastRand() % 1500) / 1000.0f;
                m_grainAge[i] = 0;
                return;
            }
        }
    }

    inline uint32_t fastRand() {
        m_rngState ^= (m_rngState << 13);
        m_rngState ^= (m_rngState >> 17);
        m_rngState ^= (m_rngState << 5);
        return m_rngState;
    }

    double m_sampleRate = 44100.0;
    std::vector<float> m_circBufferL, m_circBufferR;
    std::vector<Grain> m_grains;
    std::array<uint32_t, kMaxGrains> m_grainAge;
    std::array<float, kWindowSize> m_windowTable;
    uint32_t m_writeIdx = 0;
    uint32_t m_rngState = 0xACE1;
    float m_density = 0.2f;
    float m_spawnAccumulator = 0.0f;
};

} // namespace Aura::DSP::Effects
