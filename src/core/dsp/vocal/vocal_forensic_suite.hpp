#pragma once
#include <vector>
#include <cmath>
#include <algorithm>
#include <deque>
#include <numeric>
#include "../../audio_buffer.hpp"
#include "phonetic_aligner_kernel.hpp"

namespace Aura::DSP::Vocal {

/**
 * @class VocalForensicSuite
 * @brief Industrial-grade vocal restoration and neural lyric orchestration.
 */
class VocalForensicSuite {
public:
    VocalForensicSuite(double sampleRate = 48000.0) 
        : m_sampleRate(std::isfinite(sampleRate) && sampleRate >= 8000.0 && sampleRate <= 384000.0 ? sampleRate : 48000.0),
          m_lookaheadSamples(static_cast<uint32_t>(0.01 * m_sampleRate)) {
        m_lookaheadL.resize(m_lookaheadSamples, 0.0f);
        m_lookaheadR.resize(m_lookaheadSamples, 0.0f);
    }

    /**
     * @brief DE-ESSER: Performs sibilance reduction with industrial precision and vocal sovereignty.
     * INDUSTRIAL: Delegating signal analysis to the Rust 'VocalOrchestrator'.
     */
    void processDeEsser(float* l, float* r, uint32_t samples) {
        if (!l || samples == 0) return;
        const float attack = std::exp(-1.0f / (0.001f * static_cast<float>(m_sampleRate)));
        const float release = std::exp(-1.0f / (0.050f * static_cast<float>(m_sampleRate)));
        for (uint32_t i = 0; i < samples; ++i) {
            const float inL = std::isfinite(l[i]) ? l[i] : 0.0f;
            const float inR = r && std::isfinite(r[i]) ? r[i] : inL;
            // First difference emphasizes sibilant/high-frequency energy.
            const float hpL = inL - m_lastL_LP;
            const float hpR = inR - m_lastR_LP;
            m_lastL_LP = inL; m_lastR_LP = inR;
            const float detector = std::max(std::fabs(hpL), std::fabs(hpR));
            const float coeff = detector > m_atten ? attack : release;
            m_atten = coeff * m_atten + (1.0f - coeff) * detector;
            const float reduction = 1.0f - std::clamp((m_atten - 0.12f) * 1.8f, 0.0f, 0.65f);
            l[i] = std::isfinite(inL * reduction) ? inL * reduction : 0.0f;
            if (r) r[i] = std::isfinite(inR * reduction) ? inR * reduction : 0.0f;
        }
    }

    /**
     * @brief GAIN RIDER: Performs automated volume leveling with industrial precision.
     * INDUSTRIAL: Using Rust for robust and perfectly timed gain riding.
     */
    void processGainRider(float* l, float* r, uint32_t samples, float targetRMS = 0.2f) {
        if (!l || samples == 0) return;
        targetRMS = std::clamp(std::isfinite(targetRMS) ? targetRMS : 0.2f, 0.01f, 1.0f);
        double energy = 0.0;
        for (uint32_t i = 0; i < samples; ++i) {
            const float v = std::isfinite(l[i]) ? l[i] : 0.0f;
            energy += static_cast<double>(v) * v;
            if (r && std::isfinite(r[i])) energy += static_cast<double>(r[i]) * r[i];
        }
        const double divisor = static_cast<double>(samples) * (r ? 2.0 : 1.0);
        const float rms = static_cast<float>(std::sqrt(energy / std::max(1.0, divisor)));
        const float desired = std::clamp(targetRMS / std::max(rms, 1.0e-4f), 0.25f, 4.0f);
        m_currentGain += (desired - m_currentGain) * 0.15f;
        for (uint32_t i = 0; i < samples; ++i) {
            l[i] = std::isfinite(l[i]) ? l[i] * m_currentGain : 0.0f;
            if (r) r[i] = std::isfinite(r[i]) ? r[i] * m_currentGain : 0.0f;
        }
    }

    // --- PHASE 47: NEURAL LYRIC BRIDGE ---
    /**
     * @brief PHONETIC SYNC: Orchestrates neural lyric alignment with absolute precision.
     * INDUSTRIAL: Delegating phonetic analysis to the Rust 'VocalOrchestrator'.
     */
    void updatePhoneticSync(const std::vector<::Aura::Core::DSP::Vocal::PhoneticAlignerKernel::Phoneme>& phonemes, const float* env, uint32_t sz) {
        if (phonemes.empty() || !env || sz == 0) { m_phoneticConfidence = 0.0f; return; }
        double energy = 0.0;
        uint32_t finite = 0;
        for (uint32_t i = 0; i < sz; ++i) if (std::isfinite(env[i])) { energy += std::clamp(env[i], 0.0f, 1.0f); ++finite; }
        const float mean = finite ? static_cast<float>(energy / finite) : 0.0f;
        const float expectedMs = std::accumulate(phonemes.begin(), phonemes.end(), 0.0f,
            [](float sum, const auto& p) { return sum + std::max(0.0f, p.idealDurationMs); });
        const float observedMs = static_cast<float>(sz) * 512.0f / static_cast<float>(m_sampleRate) * 1000.0f;
        const float durationFit = expectedMs > 1.0f ? std::exp(-std::fabs(observedMs - expectedMs) / expectedMs) : 0.0f;
        m_phoneticConfidence = std::clamp(0.5f * mean + 0.5f * durationFit, 0.0f, 1.0f);
    }

    float getPhoneticConfidence() const { return m_phoneticConfidence; }

private:
    double m_sampleRate;
    uint32_t m_lookaheadSamples;
    std::deque<float> m_lookaheadL, m_lookaheadR;
    float m_currentGain = 1.0f;
    float m_lastL_LP = 0, m_lastR_LP = 0;
    float m_atten = 1.0f;
    float m_phoneticConfidence = 1.0f;
};

} // namespace Aura::DSP::Vocal
