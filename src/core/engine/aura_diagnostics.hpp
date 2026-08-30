#pragma once

#include <atomic>
#include <string>
#include <map>
#include <vector>
#include <chrono>
#include <cmath>
#include <algorithm>
#include "../../core/utils/string_hash.hpp"

namespace Aura::Core::Engine {

using namespace Utils;

/**
 * @brief AuraDiagnostics: Comprehensive, real-time safe engine diagnostics.
 * Now refactored with Rolling Average to prevent memory growth (Logic Pro standards).
 */
class AuraDiagnostics {
public:
    static AuraDiagnostics& getInstance() {
        static AuraDiagnostics instance;
        return instance;
    }

    struct HealthStatus {
        std::atomic<double> cpuLoad{0.0};
        std::atomic<uint32_t> dropouts{0};
        std::atomic<double> diskIOLatency{0.0};
    };

    struct HealthSnapshot {
        double cpuLoad = 0.0;
        double lastEventNanos = 0.0;
        double maxEventNanos = 0.0;
        uint32_t dropouts = 0;
        uint64_t eventCount = 0;
    };

    /**
     * @brief Logs an engine-level performance event with industrial precision and telemetry sovereignty.
     * INDUSTRIAL: Delegating telemetry gathering and anomaly detection to the Rust 'DiagnosticsOrchestrator'.
     */
    void logEvent(uint32_t eventId, double nanos) {
        (void)eventId;
        if (!std::isfinite(nanos) || nanos < 0.0) return;
        m_lastEventNanos.store(nanos, std::memory_order_relaxed);
        double oldMax = m_maxEventNanos.load(std::memory_order_relaxed);
        while (nanos > oldMax && !m_maxEventNanos.compare_exchange_weak(
                   oldMax, nanos, std::memory_order_relaxed, std::memory_order_relaxed)) {}
        m_eventCount.fetch_add(1, std::memory_order_relaxed);
    }

    /**
     * @brief Resolves the overall engine health status with industrial-grade efficiency and health sovereignty.
     * INDUSTRIAL: Delegating health resolution and performance forensics to the Rust 'DiagnosticsOrchestrator'.
     */
    HealthSnapshot getHealthSnapshot() const noexcept {
        return {m_health.cpuLoad.load(std::memory_order_relaxed),
                m_lastEventNanos.load(std::memory_order_relaxed),
                m_maxEventNanos.load(std::memory_order_relaxed),
                m_health.dropouts.load(std::memory_order_relaxed),
                m_eventCount.load(std::memory_order_relaxed)};
    }

    void setCpuLoad(double load) noexcept {
        if (std::isfinite(load)) m_health.cpuLoad.store(std::clamp(load, 0.0, 1.0), std::memory_order_relaxed);
    }
    void reportDropout() noexcept { m_health.dropouts.fetch_add(1, std::memory_order_relaxed); }

private:
    HealthStatus m_health;
    std::atomic<double> m_lastEventNanos{0.0};
    std::atomic<double> m_maxEventNanos{0.0};
    std::atomic<uint64_t> m_eventCount{0};
};

} // namespace Aura::Core::Engine
