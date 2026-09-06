#pragma once
#include <vector>
#include <cmath>
#include <algorithm>
#include <functional>

namespace Aura::DSP::Spatial {

/**
 * @class HolographicPanner
 * @brief Object-based 3D spatial processor with binaural HRTF simulation.
 * Essential for cinematic and immersive audio production.
 */
class HolographicPanner {
public:
    static constexpr uint32_t kMaxHrtfTaps = 128;
    using HrtfProvider = std::function<bool(float azimuth, float elevation, float distance,
                                            float* left, float* right, uint32_t& taps)>;
    // A host/plugin can provide measured left/right impulse responses.  The
    // fallback remains deterministic when no kernel is installed.
    bool setHrtfKernel(const float* left, const float* right, uint32_t taps) noexcept {
        if (!left || !right || taps == 0 || taps > kMaxHrtfTaps) return false;
        for (uint32_t i = 0; i < taps; ++i) {
            if (!std::isfinite(left[i]) || !std::isfinite(right[i])) return false;
        }
        m_hrtfTaps = taps;
        std::copy(left, left + taps, m_hrtfL);
        std::copy(right, right + taps, m_hrtfR);
        std::fill(m_hrtfL + taps, m_hrtfL + kMaxHrtfTaps, 0.0f);
        std::fill(m_hrtfR + taps, m_hrtfR + kMaxHrtfTaps, 0.0f);
        std::fill(std::begin(m_history), std::end(m_history), 0.0f);
        m_historyPtr = 0;
        return true;
    }

    void clearHrtfKernel() noexcept {
        m_hrtfTaps = 0;
        std::fill(std::begin(m_hrtfL), std::end(m_hrtfL), 0.0f);
        std::fill(std::begin(m_hrtfR), std::end(m_hrtfR), 0.0f);
    }

    bool hasHrtfKernel() const noexcept { return m_hrtfTaps != 0; }

    // Provider lookup is deliberately control-plane. The returned IR is
    // copied into the bounded realtime kernel; no filesystem/database access
    // occurs from process().
    bool loadHrtfFromProvider(const HrtfProvider& provider,
                              float azimuth, float elevation, float distance) {
        if (!provider || !std::isfinite(azimuth) || !std::isfinite(elevation) ||
            !std::isfinite(distance) || distance < 0.0f) return false;
        float left[kMaxHrtfTaps]{}, right[kMaxHrtfTaps]{};
        uint32_t taps = 0;
        if (!provider(azimuth, elevation, distance, left, right, taps)) return false;
        return setHrtfKernel(left, right, taps);
    }

    HolographicPanner(double sampleRate = 48000.0) : m_sampleRate(sampleRate) {
        std::fill(std::begin(m_delayL), std::end(m_delayL), 0.0f);
        std::fill(std::begin(m_delayR), std::end(m_delayR), 0.0f);
    }

    void setSampleRate(double sampleRate) noexcept {
        if (!std::isfinite(sampleRate) || sampleRate <= 0.0) return;
        m_sampleRate = sampleRate;
        std::fill(std::begin(m_delayL), std::end(m_delayL), 0.0f);
        std::fill(std::begin(m_delayR), std::end(m_delayR), 0.0f);
        std::fill(std::begin(m_history), std::end(m_history), 0.0f);
        m_ptr = 0;
        m_historyPtr = 0;
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
        x = std::isfinite(x) ? std::clamp(x, -1.0f, 1.0f) : 0.0f;
        y = std::isfinite(y) ? std::clamp(y, -1.0f, 1.0f) : 0.0f;
        z = std::isfinite(z) ? std::clamp(z, -1.0f, 1.0f) : 0.0f;

        // Bounded binaural approximation: equal-power azimuth, distance
        // attenuation and a short interaural delay.  It is deterministic,
        // allocation-free and provides a useful native fallback when a full
        // HRTF provider is unavailable.
        const double sampleRate = std::isfinite(m_sampleRate) && m_sampleRate > 0.0
            ? m_sampleRate
            : 48'000.0;
        const float distance = std::clamp(1.0f - 0.35f * std::max(0.0f, z) - 0.15f * std::abs(y), 0.25f, 1.0f);
        const float angle = (x + 1.0f) * 0.25f * 3.14159265358979323846f;
        const float leftGain = std::cos(angle) * distance;
        const float rightGain = std::sin(angle) * distance;
        const uint32_t delaySamples = std::min<uint32_t>(511u,
            static_cast<uint32_t>(std::abs(x) * 0.0007 * sampleRate));

        for (uint32_t i = 0; i < samples; ++i) {
            const float inL = l[i];
            const float inR = r[i];
            const float mono = 0.5f * ((std::isfinite(inL) ? inL : 0.0f)
                                     + (std::isfinite(inR) ? inR : 0.0f));
            const uint32_t slot = m_ptr++ % 512u;
            m_delayL[slot] = mono;
            m_delayR[slot] = mono;
            const uint32_t delayed = (slot + 512u - delaySamples) % 512u;
            if (m_hrtfTaps != 0) {
                m_history[m_historyPtr++ % kMaxHrtfTaps] = mono;
                float outL = 0.0f, outR = 0.0f;
                for (uint32_t tap = 0; tap < m_hrtfTaps; ++tap) {
                    const uint32_t index = (m_historyPtr + kMaxHrtfTaps - 1u - tap) % kMaxHrtfTaps;
                    outL += m_history[index] * m_hrtfL[tap];
                    outR += m_history[index] * m_hrtfR[tap];
                }
                l[i] = std::isfinite(outL) ? outL * distance : 0.0f;
                r[i] = std::isfinite(outR) ? outR * distance : 0.0f;
            } else {
                l[i] = std::isfinite(m_delayL[delayed] * leftGain) ? m_delayL[delayed] * leftGain : 0.0f;
                r[i] = std::isfinite(m_delayR[delayed] * rightGain) ? m_delayR[delayed] * rightGain : 0.0f;
            }
        }
    }

private:
    double m_sampleRate;
    float m_delayL[512], m_delayR[512];
    float m_hrtfL[kMaxHrtfTaps]{}, m_hrtfR[kMaxHrtfTaps]{};
    float m_history[kMaxHrtfTaps]{};
    uint32_t m_hrtfTaps = 0;
    uint32_t m_historyPtr = 0;
    uint32_t m_ptr = 0;
};

} // namespace Aura::DSP::Spatial
