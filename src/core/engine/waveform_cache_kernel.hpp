#pragma once

#include <stdint.h>
#include <vector>
#include <mutex>
#include <atomic>
#include <cmath>
#include <algorithm>

namespace Aura::Core::Engine {

/**
 * @struct PeakPair
 * @brief Min/Max peaks for a high-resolution waveform display.
 */
struct PeakPair {
    float min;
    float max;
};

/**
 * @class WaveformCacheKernel
 * @brief Clinical-grade waveform peak generation and caching.
 * Provides Ardour-level visual fidelity for multi-terabyte assets.
 */
class WaveformCacheKernel {
public:
    WaveformCacheKernel(uint32_t samplesPerPixel = 256) 
        : m_samplesPerPixel(std::max<uint32_t>(1u, samplesPerPixel)) {}

    /**
     * @brief Generate peak data from a raw buffer.
     */
    void generateForBlock(const float* data, uint32_t size) {
        if (!data || size == 0) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        for (uint32_t i = 0; i < size; ++i) {
            const float sample = std::isfinite(data[i]) ? data[i] : 0.0f;
            if (m_sampleCounter == 0) { m_currentMin = sample; m_currentMax = sample; }
            else { m_currentMin = std::min(m_currentMin, sample); m_currentMax = std::max(m_currentMax, sample); }
            if (++m_sampleCounter >= m_samplesPerPixel) {
                m_peaks.push_back({m_currentMin, m_currentMax});
                m_sampleCounter = 0;
            }
        }
    }

    void flush() {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (m_sampleCounter == 0) return;
        m_peaks.push_back({m_currentMin, m_currentMax});
        m_sampleCounter = 0;
    }

    void clear() {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_peaks.clear(); m_sampleCounter = 0; m_currentMin = 0.0f; m_currentMax = 0.0f;
    }


    const std::vector<PeakPair>& getPeaks() const { return m_peaks; }

    std::vector<PeakPair> copyPeaks() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_peaks;
    }

    size_t peakCount() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_peaks.size();
    }

    bool getPeak(size_t index, PeakPair& out) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (index >= m_peaks.size()) return false;
        out = m_peaks[index];
        return true;
    }

private:
    uint32_t m_samplesPerPixel;
    uint32_t m_sampleCounter = 0;
    float m_currentMin = 0.0f;
    float m_currentMax = 0.0f;
    std::vector<PeakPair> m_peaks;
    mutable std::mutex m_mutex;
};

} // namespace Aura::Core::Engine
