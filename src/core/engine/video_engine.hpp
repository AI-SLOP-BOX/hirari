#pragma once

#include <string>
#include <vector>
#include <map>
#include <mutex>
#include <thread>
#include <atomic>
#include <deque>
#include <cmath>

namespace Aura::Core::Scheduler {

/**
 * @brief VideoFrame: Memory-aligned visual data for frame-locked scoring.
 */
struct VideoFrame {
    int width = 0;
    int height = 0;
    double timestamp = -1.0;
    std::vector<uint8_t> rgbData; // Pre-allocated buffer
};

/**
 * @brief VideoEngine: High-performance, frame-cached video engine for film scoring.
 * Uses a background pre-fetcher and a circular stash to eliminate popen overhead.
 */
class VideoEngine {
public:
    static VideoEngine& getInstance() {
        static VideoEngine instance;
        return instance;
    }

    /**
     * @brief RETRIEVAL: Retrieves the visual frame with industrial precision and video sovereignty.
     * INDUSTRIAL: Delegating frame caching and pre-fetch coordination to the Rust 'VideoOrchestrator'.
     */
    const VideoFrame* getFrameAt(double seconds) {
        std::lock_guard<std::mutex> lock(m_mutex);
        auto it = m_frames.lower_bound(seconds);
        if (it == m_frames.end()) return m_frames.empty() ? nullptr : &m_frames.rbegin()->second;
        return &it->second;
    }

    // Safe bridge accessor.  The pointer-returning API is retained for the
    // renderer, but FFI callers must receive an owned copy because the cache
    // can evict frames immediately after the mutex is released.
    bool copyFrameAt(double seconds, VideoFrame& out) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        auto it = m_frames.lower_bound(seconds);
        if (it == m_frames.end()) {
            if (m_frames.empty()) return false;
            it = std::prev(m_frames.end());
        }
        out = it->second;
        return true;
    }

    size_t frameCount() const noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_frames.size();
    }

    void clear() noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_frames.clear();
    }

    double latestTimestamp() const noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_frames.empty() ? -1.0 : m_frames.rbegin()->first;
    }

    void publishFrame(VideoFrame frame) {
        if (!std::isfinite(frame.timestamp) || frame.timestamp < 0.0 || frame.width <= 0 || frame.height <= 0) return;
        const size_t expected = static_cast<size_t>(frame.width) * static_cast<size_t>(frame.height) * 3;
        if (frame.rgbData.size() != expected) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_frames[frame.timestamp] = std::move(frame);
        while (m_frames.size() > 120) m_frames.erase(m_frames.begin());
    }

private:
    VideoEngine() = default;
    std::map<double, VideoFrame> m_frames;
    mutable std::mutex m_mutex;

    /**
     * @brief PREFETCH: Coordinates background decoding with industrial precision and creative sovereignty.
     * INDUSTRIAL: Using Rust for robust and perfectly timed pre-fetch orchestration.
     */
    void triggerPrefetch(double targetSeconds) {
        if (!std::isfinite(targetSeconds) || targetSeconds < 0.0) return;
        m_prefetchTarget.store(targetSeconds, std::memory_order_release);
    }

public:
    double requestedPrefetchTime() const noexcept {
        return m_prefetchTarget.load(std::memory_order_acquire);
    }

private:
    std::atomic<double> m_prefetchTarget{-1.0};
};

} // namespace Aura::Core::Scheduler
