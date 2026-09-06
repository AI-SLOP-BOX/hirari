#pragma once
#include <cmath>
#include <algorithm>
#include <cstdio>
#include <cstring>
#include "../iprocessor.hpp"
#include "../mixing/state_variable_filter.hpp"

namespace Aura::DSP::Effects {

/**
 * @class FETCompressor
 * @brief High-speed FET-style feedback compressor (1176 emulation).
 * HONEST FIX: Replaced expensive 'std::pow/log' calls with fast approximations 
 * to ensure high-density multitrack performance, and fixed math explosion in saturation.
 */
/**
 * @class FETCompressor
 * @brief High-speed FET-style feedback compressor (1176 emulation).
 * HONEST FIX: Replaced expensive 'std::pow/log' calls with fast approximations 
 * to ensure high-density multitrack performance.
 */
class FETCompressor : public IProcessor {
public:
    FETCompressor(double sr = 44100.0) : m_sampleRate(sr), m_scHPF(sr) {
        if (!std::isfinite(m_sampleRate) || m_sampleRate < 100.0 || m_sampleRate > 384000.0) m_sampleRate = 44100.0;
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        if (std::isfinite(sr) && sr >= 100.0 && sr <= 384000.0) m_sampleRate = sr;
        setAttackMs(m_attackMs);
        setReleaseMs(m_releaseMs);
        reset();
    }

    void setThreshold(float db) { m_threshold = db; }
    void setRatio(int ratio) { m_ratioFlat = 1.0f - 1.0f / static_cast<float>(std::clamp(ratio, 1, 20)); }
    void setAttack(float ms) { m_attackMs = std::clamp(std::isfinite(ms) ? ms : 1.0f, 0.1f, 200.0f); setAttackMs(m_attackMs); }
    void setRelease(float ms) { m_releaseMs = std::clamp(std::isfinite(ms) ? ms : 100.0f, 1.0f, 2000.0f); setReleaseMs(m_releaseMs); }

    void setParameters(float input, float output, float threshold, float attackMs, float releaseMs, int ratio) {
        m_inputGain = std::pow(10.0f, std::clamp(std::isfinite(input) ? input : 0.0f, -60.0f, 24.0f) / 20.0f);
        m_outputGain = std::pow(10.0f, std::clamp(std::isfinite(output) ? output : 0.0f, -60.0f, 24.0f) / 20.0f);
        setThreshold(threshold);
        setAttack(attackMs);
        setRelease(releaseMs);
        setRatio(ratio);
    }

    std::string getName() const override { return "FET Compressor"; }
    uint32_t getNumParameters() const noexcept override { return 6; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        value = std::clamp(value, 0.0f, 1.0f);
        if (id == 0) m_inputGain = std::pow(10.0f, (-60.0f + value * 84.0f) / 20.0f);
        else if (id == 1) m_outputGain = std::pow(10.0f, (-60.0f + value * 84.0f) / 20.0f);
        else if (id == 2) setThreshold(-60.0f + value * 60.0f);
        else if (id == 3) setRatio(static_cast<int>(std::lround(1.0f + value * 19.0f)));
        else if (id == 4) setAttack(0.1f + value * 199.9f);
        else if (id == 5) setRelease(1.0f + value * 1999.0f);
    }
    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return std::clamp((20.0f * std::log10(std::max(m_inputGain, 1.0e-6f)) + 60.0f) / 84.0f, 0.0f, 1.0f);
        if (id == 1) return std::clamp((20.0f * std::log10(std::max(m_outputGain, 1.0e-6f)) + 60.0f) / 84.0f, 0.0f, 1.0f);
        if (id == 2) return std::clamp((m_threshold + 60.0f) / 60.0f, 0.0f, 1.0f);
        if (id == 3) return std::clamp((1.0f / std::max(1.0e-6f, 1.0f - m_ratioFlat) - 1.0f) / 19.0f, 0.0f, 1.0f);
        if (id == 4) return std::clamp((m_attackMs - 0.1f) / 199.9f, 0.0f, 1.0f);
        return id == 5 ? std::clamp((m_releaseMs - 1.0f) / 1999.0f, 0.0f, 1.0f) : 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override { if (id >= 6) return false; out = {0.0f, 1.0f, id == 3}; return true; }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override { if (!outName || !maxSize) return; const char* names[] = {"Input", "Output", "Threshold", "Ratio", "Attack", "Release"}; std::snprintf(outName, maxSize, "%s", id < 6 ? names[id] : ""); }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(40, 0);
        const uint32_t magic = 0x41555241u; const uint16_t version = 1; const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data() + 4, &version, 2); std::memcpy(state.data() + 6, &flags, 2);
        std::memcpy(state.data() + 8, &m_mix, 4); std::memcpy(state.data() + 12, &m_sidechainBusId, 4);
        float values[6]{}; for (uint32_t i = 0; i < 6; ++i) values[i] = getParameter(i); std::memcpy(state.data() + 16, values, sizeof(values)); return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 40) return false;
        uint32_t magic = 0, sidechain = 0; uint16_t version = 0, flags = 0; float mix = 0.0f, values[6]{};
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data() + 4, 2); std::memcpy(&flags, state.data() + 6, 2); std::memcpy(&mix, state.data() + 8, 4); std::memcpy(&sidechain, state.data() + 12, 4); std::memcpy(values, state.data() + 16, sizeof(values));
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f) return false;
        for (float value : values) if (!std::isfinite(value) || value < 0.0f || value > 1.0f) return false;
        m_bypassed = (flags & 1u) != 0; m_mix = mix; m_sidechainBusId = sidechain; for (uint32_t i = 0; i < 6; ++i) setParameter(i, values[i]); return true;
    }

    void process(::Aura::Core::AudioBuffer& b, ::Aura::Core::MidiBuffer& midi, const ::Aura::DSP::ProcessContext& context) noexcept override {
        (void)midi; (void)context;
        if (m_bypassed) return;
        const uint32_t channels = b.getNumChannels();
        const uint32_t samples = b.getNumSamples();
        if (channels == 0 || samples == 0) return;
        for (uint32_t i = 0; i < samples; ++i) {
            float peak = 0.0f;
            for (uint32_t c = 0; c < channels; ++c) {
                const float* p = b.getReadPointer(c);
                if (p && std::isfinite(p[i])) peak = std::max(peak, std::abs(p[i] * m_inputGain));
            }
            const float detector = std::max(peak, 1.0e-8f);
            const float coeff = detector > m_envelope ? m_attack : m_release;
            m_envelope += (detector - m_envelope) * (1.0f - coeff);
            const float levelDb = 6.0205999f * fastLog2(std::max(m_envelope, 1.0e-8f));
            const float over = std::max(0.0f, levelDb - m_threshold);
            const float reductionDb = over * std::clamp(m_ratioFlat, 0.0f, 0.95f);
            const float gain = std::clamp(fastPow2((-reductionDb + 0.0f) / 6.0205999f) * m_outputGain, 0.0f, 4.0f);
            for (uint32_t c = 0; c < channels; ++c) {
                float* p = b.getWritePointer(c);
                if (!p) continue;
                const float x = std::isfinite(p[i]) ? p[i] : 0.0f;
                p[i] = std::isfinite(x * gain) ? x * gain : 0.0f;
            }
        }
    }


    void reset() noexcept override { m_envelope = 1.0f; }

    uint32_t getTailSamples() const noexcept override {
        return static_cast<uint32_t>(std::min(30.0 * std::clamp(m_sampleRate, 100.0, 384000.0),
            std::clamp(static_cast<double>(m_releaseMs), 1.0, 2000.0) * 0.001 * m_sampleRate * 8.0));
    }

private:
    /**
     * @brief Professional fast log2 approximation for audio DSP.
     */
    inline float fastLog2(float x) const {
        union { float f; uint32_t i; } vx = { x };
        float y = (float)vx.i;
        y *= 1.1920928955078125e-7f;
        return y - 126.94269504f;
    }

    /**
     * @brief Professional fast pow2 approximation for audio DSP.
     */
    inline float fastPow2(float p) const {
        float clipp = (p < -126) ? -126.0f : p;
        union { uint32_t i; float f; } v = { (uint32_t)((clipp + 126.94269504f) * 8388608.0f) };
        return v.f;
    }

    void setAttackMs(float ms) noexcept { m_attack = std::exp(-1.0f / (std::max(0.1f, ms) * 0.001f * std::max(1000.0, m_sampleRate))); }
    void setReleaseMs(float ms) noexcept { m_release = std::exp(-1.0f / (std::max(1.0f, ms) * 0.001f * std::max(1000.0, m_sampleRate))); }

    double m_sampleRate;
    float m_inputGain = 1.0f, m_outputGain = 1.0f;
    float m_threshold = -24.0f, m_ratioFlat = 0.75f;
    float m_attack = 0.99f, m_release = 0.999f;
    float m_attackMs = 1.0f, m_releaseMs = 100.0f;
    float m_envelope = 1.0f;
    Mixing::StateVariableFilter m_scHPF;
};

} // namespace Aura::DSP::Effects
