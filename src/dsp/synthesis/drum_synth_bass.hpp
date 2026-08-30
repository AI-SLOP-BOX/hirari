#pragma once

#include <cmath>
#include <atomic>
#include <algorithm>

namespace Aura::Core::DSP::Synthesis {

/**
 * @brief DrumSynthBass: Professional SOTA Kick Designer.
 * HONEST FIX: Replaced slow non-real-time RNG and floor() with SIMD-ready math.
 */
class DrumSynthBass {
public:
    explicit DrumSynthBass(double sr) : m_sampleRate(sr) {}

    void trigger(float pitchStart = 150.0f, float decay = 0.5f, float saturation = 1.2f) {
        m_nextPitchStart.store(pitchStart, std::memory_order_relaxed);
        m_nextDecay.store(decay, std::memory_order_relaxed);
        m_nextSaturation.store(saturation, std::memory_order_relaxed);
        m_shouldTrigger.store(true, std::memory_order_release);
    }

    void render(float* l, float* r, size_t numFrames) {
        if (!l || !r || numFrames == 0) return;
        if (m_shouldTrigger.exchange(false, std::memory_order_acq_rel)) {
            m_pStart = std::clamp(m_nextPitchStart.load(std::memory_order_relaxed), 40.0f, 400.0f);
            m_pDecay = std::clamp(m_nextDecay.load(std::memory_order_relaxed), 0.05f, 2.0f);
            m_pSat = std::clamp(m_nextSaturation.load(std::memory_order_relaxed), 0.1f, 8.0f);
            m_phase = 0.0; m_envPos = 0.0; m_pitchEnv = 1.0f; m_isActive.store(true, std::memory_order_release);
        }
        if (!m_isActive.load(std::memory_order_acquire)) { std::fill(l, l + numFrames, 0.0f); std::fill(r, r + numFrames, 0.0f); return; }
        for (size_t i = 0; i < numFrames; ++i) {
            const float env = std::exp(-static_cast<float>(m_envPos) / (m_pDecay * static_cast<float>(m_sampleRate)));
            const float pitch = 45.0f + m_pStart * std::exp(-static_cast<float>(m_envPos) / (0.045f * static_cast<float>(m_sampleRate)));
            m_phase += 2.0 * M_PI * pitch / m_sampleRate;
            if (m_phase > 2.0 * M_PI) m_phase -= 2.0 * M_PI;
            const float raw = static_cast<float>(std::sin(m_phase) * env);
            const float shaped = std::tanh(raw * m_pSat) * 0.85f;
            l[i] = std::isfinite(shaped) ? shaped : 0.0f;
            r[i] = l[i];
            m_envPos += 1.0;
        }
        if (std::exp(-m_envPos / (m_pDecay * m_sampleRate)) < 1.0e-4) m_isActive.store(false, std::memory_order_release);
    }


private:
    double m_sampleRate;
    double m_phase = 0.0, m_envPos = 0.0;
    float m_pStart = 150.f, m_pDecay = 0.5f, m_pSat = 1.2f;
    float m_pitchEnv = 1.0f, m_pitchDropCoef = 1.0f;

    std::atomic<float> m_nextPitchStart{150.0f}, m_nextDecay{0.5f}, m_nextSaturation{1.2f};
    std::atomic<bool> m_shouldTrigger{false}, m_isActive{false};
};

} // namespace Aura::Core::DSP::Synthesis
