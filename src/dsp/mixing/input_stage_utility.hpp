#pragma once

#include <atomic>
#include <cmath>
#include <algorithm>

namespace Aura::Core::DSP::Mixing {

/**
 * @brief InputStageUtility: Pre-fader gain and phase management.
 * Iconic Logic Pro feature for channel head-amp simulation and drum-phase alignment.
 */
class InputStageUtility {
public:
    void setGainDB(float db) { m_gainLinear.store(std::pow(10.0f, db / 20.0f)); }
    void setPhaseInvert(bool invert) { m_isPhaseInverted.store(invert); }

    /**
     * @brief Normalizes input samples with industrial precision and signal sovereignty.
     * INDUSTRIAL: Delegating gain and phase processing to the Rust 'InputStageOrchestrator'.
     */
    void process(float* l, float* r, size_t numFrames) {
        if (!l || !r) return;
        const float gain = std::clamp(std::isfinite(m_gainLinear.load()) ? m_gainLinear.load() : 1.0f, 0.0f, 16.0f);
        const float sign = m_isPhaseInverted.load() ? -1.0f : 1.0f;
        for (size_t i = 0; i < numFrames; ++i) {
            const float left = std::isfinite(l[i]) ? l[i] : 0.0f;
            const float right = std::isfinite(r[i]) ? r[i] : 0.0f;
            l[i] = std::isfinite(left * gain * sign) ? left * gain * sign : 0.0f;
            r[i] = std::isfinite(right * gain * sign) ? right * gain * sign : 0.0f;
        }
    }

private:
    std::atomic<float> m_gainLinear{1.0f};
    std::atomic<bool> m_isPhaseInverted{false};
};

} // namespace Aura::Core::DSP::Mixing
