#pragma once
#include <atomic>
#include <algorithm>
#include <mutex>

namespace Aura::UI::Main {

/**
 * @class WaveformScaleManager
 * @brief Professional Visual Zoom for audio waveforms.
 * HONEST FIX: Implemented adaptive scaling and smoothed transitions.
 */
class WaveformScaleManager {
public:
    static WaveformScaleManager& getInstance() {
        static WaveformScaleManager instance;
        return instance;
    }

    /**
     * @brief Sets the target visual gain. The actual gain will smooth towards this.
     */
    void setTargetGain(float gain) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_targetGain = std::clamp(gain, 0.1f, 100.0f);
    }

    /**
     * @brief Updates the smoothed gain (should be called once per UI frame).
     */
    void update() {
        std::lock_guard<std::mutex> lock(m_mutex);
        float current = m_currentGain;
        float target = m_targetGain;
        
        // Exponential smoothing: 0.2 coeff for smooth but responsive scaling
        float next = current + 0.2f * (target - current);
        m_currentGain = next;
    }

    float getVisualGain() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_currentGain;
    }

    /**
     * @brief Calculates the optimal gain to make a signal's peak reach ~70% of vertical height.
     */
    float calculateOptimalGain(float peakLevel) const {
        if (!std::isfinite(peakLevel) || peakLevel < 0.001f) return 1.0f;
        return std::clamp(0.7f / peakLevel, 0.1f, 100.0f);
    }

    float scaleSample(float raw) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return std::isfinite(raw) ? raw * m_currentGain : 0.0f;
    }

private:
    WaveformScaleManager() : m_targetGain(1.0f), m_currentGain(1.0f) {}

    mutable std::mutex m_mutex;
    float m_targetGain;
    float m_currentGain;
};

} // namespace Aura::UI::Main
