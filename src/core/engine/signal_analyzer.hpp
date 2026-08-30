#pragma once
#include <stdint.h>
#include <vector>
#include <atomic>
#include <algorithm>
#include <cmath>
#include <memory>
#include "../audio_buffer.hpp"

namespace Aura::Core::Engine {

struct MeterData {
    float peak[2];
    float correlation;
    float lufsIntegrated;
};

/**
 * @class SignalAnalyzer
 * @brief Triple-buffered signal analysis for UI visualization.
 * HONEST FIX: Renamed from 'SignalForensicSuite' and purged marketing labels.
 */
class SignalAnalyzer {
public:
    SignalAnalyzer(uint32_t fftSize = 2048) : m_fftSize(fftSize) {
        for (int i = 0; i < 3; ++i) {
            m_buffers[i] = std::make_unique<std::vector<float>>(fftSize, 0.0f);
        }
        m_writeIdx.store(0);
        m_latestIdx.store(0);
    }

    /**
     * @brief Writes an audio block to the next available analysis buffer.
     */
    void process(const AudioBuffer& buffer) {
        if (buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0 || m_fftSize == 0) return;
        const uint32_t latest = m_latestIdx.load(std::memory_order_acquire);
        const uint32_t ui = m_uiIdx.load(std::memory_order_acquire);
        uint32_t write = m_writeIdx.load(std::memory_order_relaxed) % 3u;
        for (uint32_t attempt = 0; attempt < 3 && (write == latest || write == ui); ++attempt)
            write = (write + 1u) % 3u;
        if (write == latest || write == ui) return;

        auto& frame = *m_buffers[write];
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2u);
        const uint32_t samples = std::min<uint32_t>(buffer.getNumSamples(), m_fftSize);
        for (uint32_t i = 0; i < samples; ++i) {
            float value = 0.0f;
            for (uint32_t channel = 0; channel < channels; ++channel) {
                const float* input = buffer.getReadPointer(channel);
                if (input != nullptr) value += input[i];
            }
            value /= static_cast<float>(channels);
            frame[i] = std::isfinite(value) ? value : 0.0f;
        }
        std::fill(frame.begin() + samples, frame.end(), 0.0f);
        m_writeIdx.store((write + 1u) % 3u, std::memory_order_relaxed);
        m_latestIdx.store(write, std::memory_order_release);
    }

    /**
     * @brief Fetches the latest analysis frame for the UI.
     */
    bool getLatestFrame(std::vector<float>& out) {
        const uint32_t latest = m_latestIdx.load(std::memory_order_acquire) % 3u;
        m_uiIdx.store(latest, std::memory_order_release);
        const auto& frame = *m_buffers[latest];
        out.assign(frame.begin(), frame.end());
        m_uiIdx.store(99u, std::memory_order_release);
        return !out.empty();
    }


private:
    uint32_t m_fftSize;
    std::unique_ptr<std::vector<float>> m_buffers[3];
    std::atomic<uint32_t> m_writeIdx;
    std::atomic<uint32_t> m_latestIdx;
    std::atomic<uint32_t> m_uiIdx{99};
};

} // namespace Aura::Core::Engine
