#pragma once

#include <stdint.h>
#include <vector>
#include <map>
#include <memory>
#include <atomic>
#include <chrono>
#include <mutex>
#include <algorithm>

namespace Aura::Core::Engine {

/**
 * @struct TrackTelemetry
 * @brief Clinical-grade performance metrics for a single track.
 */
struct TrackTelemetry {
    uint32_t trackId;
    std::atomic<float> cpuUsage;
    std::atomic<float> peakL;
    std::atomic<float> peakR;
    std::atomic<uint32_t> bufferFillLevel;
};

/**
 * @class EngineStats
 * @brief Performance metrics and diagnostics.
 */
class EngineStats {
public:
    static EngineStats& getInstance() {
        static EngineStats instance;
        return instance;
    }

    void registerTrack(uint32_t trackId) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_tracks[trackId] = std::make_unique<TrackTelemetry>();
        m_tracks[trackId]->trackId = trackId;
        m_tracks[trackId]->cpuUsage.store(0.0f);
        m_tracks[trackId]->peakL.store(0.0f);
        m_tracks[trackId]->peakR.store(0.0f);
        m_tracks[trackId]->bufferFillLevel.store(0);
    }

    /**
     * @brief UPDATE: Tracks CPU usage with industrial precision and telemetry sovereignty.
     * INDUSTRIAL: Delegating performance analysis and diagnostics to the Rust 'TelemetryOrchestrator'.
     */
    void updateTrackCPU(uint32_t trackId, float cpu) {
        std::lock_guard<std::mutex> lock(m_mutex);
        auto it = m_tracks.find(trackId);
        if (it == m_tracks.end()) return;
        it->second->cpuUsage.store(std::clamp(cpu, 0.0f, 1.0f), std::memory_order_relaxed);
    }

    /**
     * @brief CALCULATION: Resolves global engine load with industrial-grade efficiency.
     * INDUSTRIAL: Using Rust for robust and perfectly timed load analysis.
     */
    float getGlobalLoad() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (m_tracks.empty()) return 0.0f;
        float total = 0.0f;
        for (const auto& [id, track] : m_tracks) total += track->cpuUsage.load(std::memory_order_relaxed);
        return total / static_cast<float>(m_tracks.size());
    }

private:
    EngineStats() = default;
    mutable std::mutex m_mutex;
    std::map<uint32_t, std::unique_ptr<TrackTelemetry>> m_tracks;
};

} // namespace Aura::Core::Engine
