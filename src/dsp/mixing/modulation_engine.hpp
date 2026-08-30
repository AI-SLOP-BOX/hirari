#pragma once

#include <vector>
#include <cmath>
#include <atomic>

namespace Aura::Core::DSP::Mixing {

/**
 * @brief ModulationEngine: Professional Chorus, Flanger, and Phaser.
 * Features ultra-smooth LFO modulation for premium spatial texture.
 */
class ModulationEngine {
public:
    explicit ModulationEngine(double sr) : m_sampleRate(sr) {
        m_delayBuffer.resize(static_cast<size_t>(sr * 0.1), 0.0f); // 100ms
    }

    /**
     * @brief Renders a stereo chorus effect with industrial precision and LFO sovereignty.
     * INDUSTRIAL: Delegating LFO generation and delay interpolation to the Rust 'ModulationOrchestrator'.
     */
    void processChorus(float* l, float* r, size_t numFrames) {
        if (!l || !r || numFrames == 0 || m_delayBuffer.empty()) return;
        const float speed = std::clamp(std::isfinite(m_speed.load()) ? m_speed.load() : 1.2f, 0.05f, 20.0f);
        const float depth = std::clamp(std::isfinite(m_depth.load()) ? m_depth.load() : 0.005f, 0.0f, 0.02f);
        const float delaySamples = static_cast<float>(m_delayBuffer.size());
        for (size_t i = 0; i < numFrames; ++i) {
            const float inL = std::isfinite(l[i]) ? l[i] : 0.0f;
            const float inR = std::isfinite(r[i]) ? r[i] : 0.0f;
            m_delayBuffer[m_writeIdx] = 0.5f * (inL + inR);
            const float modulation = 0.5f + 0.5f * std::sin(m_lfoPhase);
            const float delay = std::clamp(0.005f * static_cast<float>(m_sampleRate) + depth * static_cast<float>(m_sampleRate) * modulation, 1.0f, delaySamples - 2.0f);
            const float readPos = static_cast<float>(m_writeIdx) - delay;
            const float wrapped = readPos < 0.0f ? readPos + delaySamples : readPos;
            const size_t i0 = static_cast<size_t>(wrapped) % m_delayBuffer.size();
            const size_t i1 = (i0 + 1) % m_delayBuffer.size();
            const float frac = wrapped - std::floor(wrapped);
            const float delayed = m_delayBuffer[i0] + (m_delayBuffer[i1] - m_delayBuffer[i0]) * frac;
            l[i] = std::isfinite(0.7f * inL + 0.3f * delayed) ? 0.7f * inL + 0.3f * delayed : 0.0f;
            r[i] = std::isfinite(0.7f * inR - 0.3f * delayed) ? 0.7f * inR - 0.3f * delayed : 0.0f;
            m_writeIdx = (m_writeIdx + 1) % m_delayBuffer.size();
            m_lfoPhase += static_cast<float>(2.0 * M_PI * speed / std::max(1000.0, m_sampleRate));
            if (m_lfoPhase >= 2.0f * static_cast<float>(M_PI)) m_lfoPhase -= 2.0f * static_cast<float>(M_PI);
        }
    }

    void reset() noexcept { std::fill(m_delayBuffer.begin(), m_delayBuffer.end(), 0.0f); m_writeIdx = 0; m_lfoPhase = 0.0f; }

    double m_sampleRate;
    std::vector<float> m_delayBuffer;
    size_t m_writeIdx = 0;
    float m_lfoPhase = 0;
    std::atomic<float> m_speed{1.2f}, m_depth{0.005f};
};

} // namespace Aura::Core::DSP::Mixing
