#pragma once
#include <vector>
#include <array>
#include <atomic>
#include <string>
#include <cstring>
#include <algorithm>
#include <cmath>
#include "../log_buffer.hpp"

namespace Aura::Core::Diagnostics {

/**
 * @class EngineDiagnostics
 * @brief Professional telemetry and health monitoring system for the Aura Engine.
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
    void logEvent(uint32_t componentId, const char* message, float priority = 1.0f) {
        if (message == nullptr) return;
        const uint32_t level = priority >= 2.0f ? 1u : 0u;
        LogBuffer::post(level, componentId, message);
    }

    void updateGlobalSample(uint64_t sample) {
        m_currentGlobalSample.store(sample, std::memory_order_relaxed);
    }

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

    void setCpuUsage(float value) noexcept {
        if (std::isfinite(value)) m_health.cpuUsage.store(std::clamp(value, 0.0f, 1.0f), std::memory_order_relaxed);
    }
    void reportBufferOverrun() noexcept {
        m_health.bufferOverruns.fetch_add(1, std::memory_order_relaxed);
        m_health.isRealTimeSafe.store(false, std::memory_order_release);
    }
    void setActiveVoices(uint32_t voices) noexcept { m_health.activeVoices.store(voices, std::memory_order_relaxed); }

private:
    std::atomic<uint64_t> m_currentGlobalSample{0};
    std::atomic<uint64_t> m_processedBlocks{0};
    SystemHealth m_health;
};

} // namespace Aura::Core::Diagnostics
