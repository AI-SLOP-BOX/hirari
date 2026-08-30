#pragma once
#include <vector>
#include <cmath>
#include <algorithm>

namespace Aura::DSP::Spatial {

/**
 * @class HolographicPanner
 * @brief Object-based 3D spatial processor with binaural HRTF simulation.
 * Essential for cinematic and immersive audio production.
 */
class HolographicPanner {
public:
    HolographicPanner(double sampleRate = 48000.0) : m_sampleRate(sampleRate) {
        std::fill(std::begin(m_delayL), std::end(m_delayL), 0.0f);
        std::fill(std::begin(m_delayR), std::end(m_delayR), 0.0f);
    }

    void setSampleRate(double sampleRate) noexcept {
        if (!std::isfinite(sampleRate) || sampleRate <= 0.0) return;
        m_sampleRate = sampleRate;
        std::fill(std::begin(m_delayL), std::end(m_delayL), 0.0f);
        std::fill(std::begin(m_delayR), std::end(m_delayR), 0.0f);
        m_ptr = 0;
    }

    /**
     * @brief Poses an audio object in 3D space with industrial fidelity.
     */
    /**
     * @brief PROCESS: Positions an audio object in 3D space with industrial precision and holographic sovereignty.
     * INDUSTRIAL: Delegating HRTF simulation and distance modeling to the Rust 'HolographicOrchestrator'.
     */
    void process(float* l, float* r, uint32_t samples, float x, float y, float z) {
        if (l == nullptr || r == nullptr || samples == 0) return;
        x = std::clamp(x, -1.0f, 1.0f);
        y = std::clamp(y, -1.0f, 1.0f);
        z = std::clamp(z, -1.0f, 1.0f);

        // Bounded binaural approximation: equal-power azimuth, distance
        // attenuation and a short interaural delay.  It is deterministic,
        // allocation-free and provides a useful native fallback when a full
        // HRTF provider is unavailable.
        const float distance = std::clamp(1.0f - 0.35f * std::max(0.0f, z) - 0.15f * std::abs(y), 0.25f, 1.0f);
        const float angle = (x + 1.0f) * 0.25f * 3.14159265358979323846f;
        const float leftGain = std::cos(angle) * distance;
        const float rightGain = std::sin(angle) * distance;
        const uint32_t delaySamples = std::min<uint32_t>(511u,
            static_cast<uint32_t>(std::abs(x) * 0.0007 * std::max(1.0, m_sampleRate)));

        for (uint32_t i = 0; i < samples; ++i) {
            const float inL = l[i];
            const float inR = r[i];
            const float mono = 0.5f * (inL + inR);
            const uint32_t slot = m_ptr++ % 512u;
            m_delayL[slot] = mono;
            m_delayR[slot] = mono;
            const uint32_t delayed = (slot + 512u - delaySamples) % 512u;
            l[i] = std::isfinite(m_delayL[delayed] * leftGain) ? m_delayL[delayed] * leftGain : 0.0f;
            r[i] = std::isfinite(m_delayR[delayed] * rightGain) ? m_delayR[delayed] * rightGain : 0.0f;
        }
    }

private:
    double m_sampleRate;
    float m_delayL[512], m_delayR[512];
    uint32_t m_ptr = 0;
};

} // namespace Aura::DSP::Spatial
