#pragma once
#include <vector>
#include <array>
#include <atomic>
#include <string>
#include <cstring>
#include <algorithm>
#include <cmath>
#include "../log_buffer.hpp"

namespace Hirari::Core::Diagnostics {

/**
 * @class EngineDiagnostics
 * @brief Professional telemetry and health monitoring system for the Hirari Engine.
 * HONEST FIX: Purged 'Infinite Void Transcendence' and other clinical hallucinations.
 */
class EngineDiagnostics {
public:
    EngineDiagnostics() = default;
    struct SystemHealth {
        std::atomic<float> cpuUsage{0.0f};
        std::atomic<uint32_t> bufferOverruns{0};
        std::atomic<uint32_t> activeVoices{0};
        std::atomic<bool> isRealTimeSafe{true};
    };

    struct Snapshot {
        float cpuUsage = 0.0f;
        uint32_t bufferOverruns = 0;
        uint32_t activeVoices = 0;
        bool isRealTimeSafe = true;
        uint64_t globalSample = 0;
        uint64_t processedBlocks = 0;
    };

    static EngineDiagnostics& getInstance() {
        static EngineDiagnostics instance;
        return instance;
    }

    /**
     * @brief Records a technical event in the circular telemetry buffer.
     */
    // Keep the audio callback boundary free of logging and formatting. A
    // control-side poller can observe this counter through snapshot().
    void recordRealtimeBlock() noexcept {
        m_processedBlocks.fetch_add(1, std::memory_order_relaxed);
    }

    SystemHealth& getHealth() { return m_health; }
    const SystemHealth& getHealth() const { return m_health; }

    Snapshot snapshot() const noexcept {
        return {m_health.cpuUsage.load(std::memory_order_relaxed),
                m_health.bufferOverruns.load(std::memory_order_relaxed),
                m_health.activeVoices.load(std::memory_order_relaxed),
                m_health.isRealTimeSafe.load(std::memory_order_relaxed),
                m_currentGlobalSample.load(std::memory_order_relaxed),
                m_processedBlocks.load(std::memory_order_relaxed)};
    }

    void setActiveVoices(uint32_t voices) noexcept { m_health.activeVoices.store(voices, std::memory_order_relaxed); }

private:
    std::atomic<uint64_t> m_currentGlobalSample{0};
    std::atomic<uint64_t> m_processedBlocks{0};
    SystemHealth m_health;
};

} // namespace Hirari::Core::Diagnostics
