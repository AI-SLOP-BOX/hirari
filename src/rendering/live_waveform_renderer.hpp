#pragma once
#include <vector>
#include <mutex>
#include <algorithm>
#include "../graphics/graphics_kernel.hpp"

namespace Hirari::Rendering {

/**
 * @class LiveWaveformRenderer
 * @brief High-performance 'Rolling' Waveform for Real-time Recording.
 * Essential for Logic Pro parity: Waveform grows in real-time behind the playhead.
 */
class LiveWaveformRenderer {
public:
    struct Peak { float min; float max; };

    LiveWaveformRenderer(size_t maxPoints = 2048) : m_maxPoints(maxPoints) {
        m_peaks.resize(maxPoints, {0.0f, 0.0f});
    }

    /**
     * @brief PUSH PCM: Extracts peaks from a block of live audio.
     */
    void pushAudio(const float* l, const float* r, uint32_t samples) {
        if (samples == 0) return;
        
        float mn = 0, mx = 0;
        for (uint32_t i = 0; i < samples; ++i) {
            float v = (l[i] + r[i]) * 0.5f; // Mono sum for visualization
            mn = std::min(mn, v);
            mx = std::max(mx, v);
        }

        std::lock_guard<std::mutex> lock(m_mutex);
        m_peaks[m_writeIdx] = { mn, mx };
        m_writeIdx = (m_writeIdx + 1) % m_maxPoints;
        if (m_count < m_maxPoints) m_count++;
    }

    /**
     * @brief RENDER: Draws the rolling waveform path.
     */
    void render(::Hirari::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, uint32_t color) {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (m_count == 0) return;
        using namespace Graphics;
        std::vector<Vertex> path;
        path.reserve(m_count * 2);

        float midY = y + h / 2.0f;
        float scaleY = h * 0.45f;
        float dx = w / m_maxPoints;

        for (size_t i = 0; i < m_count; ++i) {
            size_t idx = (m_writeIdx - m_count + i + m_maxPoints) % m_maxPoints;
            float px = x + i * dx;
            path.push_back({px, midY + m_peaks[idx].min * scaleY, color});
            path.push_back({px, midY + m_peaks[idx].max * scaleY, color});
        }
        
        kernel.drawVertexPath(path.data(), path.size(), 1.2f);
    }

    void reset() {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_writeIdx = 0; m_count = 0;
    }

private:
    std::vector<Peak> m_peaks;
    size_t m_writeIdx = 0;
    size_t m_count = 0;
    size_t m_maxPoints;
    std::mutex m_mutex;
};

} // namespace Hirari::Rendering
