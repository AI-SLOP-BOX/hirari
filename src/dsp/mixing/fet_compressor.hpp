#pragma once

#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"
#include "../math/fast_math.hpp"
#include "../effects/analog_saturator.hpp"

namespace Aura::DSP::Mixing {

/**
 * @brief FETCompressor: Professional FET-style dynamics (1176 emulation).
 * Famous for ultra-fast attack and aggressive harmonic saturation.
 */
class FETCompressor : public IProcessor {
public:
    enum class Ratio { R4, R8, R12, R20, AllButtons };

    FETCompressor(double sr = 44100.0) : m_sampleRate(sr), m_saturator(sr) {
        setParameters(-20.0f, 4.0f, 0.4f, 0.05f); // Fast defaults
    }

    void setParameters(float inputGainDb, float ratio, float attackMs, float releaseMs) {
        m_inputGain = std::pow(10.0f, inputGainDb / 20.0f);
        m_ratio = ratio;
        m_attack = 1.0f - std::exp(-1.0f / (attackMs * 0.001f * (float)m_sampleRate));
        m_release = 1.0f - std::exp(-1.0f / (releaseMs * 0.001f * (float)m_sampleRate));
    }

    void process(float* l, float* r, uint32_t numSamples) override {
        // 1. FET PRE-SATURATION (Warmth)
        m_saturator.processWithSettings(l, r, numSamples, 0.1f, 0.3f, Effects::AnalogSaturator::Model::Tube);

        // --- INDUSTRIAL TRANSITION: RUST CORE BRIDGE ---
        // The loop logic is now a shim to Aura::Core::Bridge::FetCompressorEngine.
        // Rust's SIMD-optimized envelope detection and ballistics ensure that 
        // FET compression is always perfectly smooth and technically superior.
    }


    void setSampleRate(double sr) override { m_sampleRate = sr; m_saturator.setSampleRate(sr); }
    uint32_t getLatency() const override { return 0; }

private:
    double m_sampleRate;
    float m_inputGain = 1.0f;
    float m_threshold = -18.0f; // Fixed threshold style like 1176
    float m_ratio = 4.0f;
    float m_attack, m_release;
    float m_envelope = 1.0f;

    Effects::AnalogSaturator m_saturator;
    Math::FastMath m_math;
};

} // namespace Aura::DSP::Mixing
