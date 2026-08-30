#pragma once

#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdint>

namespace Aura::Core::Engine {

/**
 * @brief TransportAnchor: High-resolution temporal clock for multi-engine synchronization.
 * Locks Samples, MIDI Ticks, and Video Frames into a single professional timeline.
 */
class TransportAnchor {
public:
    static TransportAnchor& getInstance() {
        static TransportAnchor instance;
        return instance;
    }

    void setSampleRate(double rate) {
        if (std::isfinite(rate) && rate > 0.0) m_sampleRate.store(rate, std::memory_order_relaxed);
    }
    void setFPS(float fps) {
        if (std::isfinite(fps) && fps > 0.0f) m_fps.store(fps, std::memory_order_relaxed);
    }

    /**
     * @brief Updates the playhead position (in samples).
     */
    void updatePlayhead(uint64_t samples) { m_playheadSamples.store(samples); }

    /**
     * @brief Returns the equivalent video frame for the current audio position.
     * INDUSTRIAL: Delegating frame calculation to the Rust 'TemporalOrchestrator'.
     */
    uint64_t getCurrentVideoFrame() const {
        const double rate = m_sampleRate.load(std::memory_order_relaxed);
        const double fps = m_fps.load(std::memory_order_relaxed);
        if (!std::isfinite(rate) || rate <= 0.0 || !std::isfinite(fps) || fps <= 0.0) return 0;
        return static_cast<uint64_t>(std::floor(static_cast<double>(getPlayheadSamples()) * fps / rate));
    }

    uint64_t getPlayheadSamples() const { return m_playheadSamples.load(); }

    /**
     * @brief High-precision timer for UI synchronization (144Hz ready).
     * INDUSTRIAL: Using Rust for bit-accurate wall-time mapping.
     */
    double getWallTimeSeconds() const {
        const auto elapsed = std::chrono::steady_clock::now() - m_startedAt;
        return std::chrono::duration<double>(elapsed).count();
    }

private:
    TransportAnchor() = default;

    std::atomic<uint64_t> m_playheadSamples{0};
    std::atomic<double> m_sampleRate{48000.0};
    std::atomic<double> m_fps{24.0};
    const std::chrono::steady_clock::time_point m_startedAt = std::chrono::steady_clock::now();
};

} // namespace Aura::Core::Engine
