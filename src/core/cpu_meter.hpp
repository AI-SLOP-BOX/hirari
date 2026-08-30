#pragma once

#include <chrono>
#include <atomic>
#include <algorithm>
#include <cmath>

#if defined(__x86_64__) || defined(_M_X64)
#include <x86intrin.h>
#endif

namespace Aura::Core {

/**
 * @class CPUMeter
 * @brief Zero-System-Call CPU Performance Meter using hardware CPU cycle counters.
 * Bypasses high_resolution_clock OS kernel calls during block processing by reading
 * ARM64 (Apple Silicon) system registers or x86 RDTSC cycles directly.
 */
class CPUMeter {
public:
    CPUMeter(double sampleRate) 
        : m_sampleRate((std::isfinite(sampleRate) && sampleRate > 0.0) ? sampleRate : 44100.0)
        , m_startCycles(0) {
        m_clockFrequency = std::max<uint64_t>(1, measureClockFrequency());
    }

    void startBlock() {
        m_startCycles = readCycleCounter();
    }

    /**
     * @brief Ends the block, calculates CPU load using hardware cycles, and updates telemetry.
     */
    void endBlock(uint32_t numSamples) {
        if (numSamples == 0 || !std::isfinite(m_sampleRate) || m_sampleRate <= 0.0 ||
            m_clockFrequency == 0) {
            m_load.store(0.0f, std::memory_order_relaxed);
            m_lastDurationMs.store(0.0f, std::memory_order_relaxed);
            return;
        }
        uint64_t end = readCycleCounter();
        uint64_t cycleDuration = (end >= m_startCycles) ? (end - m_startCycles) : 0;
        
        // Calculate block duration in seconds based on calibrated CPU frequency
        double durationSeconds = static_cast<double>(cycleDuration) / m_clockFrequency;
        double maxTimeSeconds = static_cast<double>(numSamples) / m_sampleRate;
        
        const double safeLoad = (std::isfinite(durationSeconds) &&
                                 std::isfinite(maxTimeSeconds) && maxTimeSeconds > 0.0)
            ? durationSeconds / maxTimeSeconds
            : 0.0;
        float currentLoad = static_cast<float>(std::clamp(safeLoad * 100.0, 0.0, 400.0));
        
        // EMA Smoothing (alpha=0.1) for stable UI metering
        float alpha = 0.1f;
        const float previous = m_load.load(std::memory_order_relaxed);
        const float smoothed = previous + alpha * (currentLoad - previous);
        m_load.store(std::isfinite(smoothed) ? smoothed : 0.0f,
                     std::memory_order_relaxed);

        // Keep the callback side atomic-only. Consumers on the control thread
        // may publish this value to diagnostics without making the audio
        // callback enter a logger, mutex, or string-building path.
        const float durationMs = static_cast<float>(std::clamp(
            std::isfinite(durationSeconds) ? durationSeconds * 1000.0 : 0.0,
            0.0, 60'000.0));
        m_lastDurationMs.store(durationMs,
                               std::memory_order_relaxed);
    }

    float getLoad() const { return m_load.load(); }
    float getLastDurationMs() const noexcept {
        return m_lastDurationMs.load(std::memory_order_relaxed);
    }

private:
    double m_sampleRate;
    uint64_t m_startCycles;
    uint64_t m_clockFrequency;
    std::atomic<float> m_load{0.0f};
    std::atomic<float> m_lastDurationMs{0.0f};

    static inline uint64_t readCycleCounter() {
#if defined(__arm64__) || defined(__aarch64__)
        uint64_t val;
        // Read virtual counter register on ARM64 (Apple Silicon)
        asm volatile("mrs %0, cntvct_el0" : "=r" (val));
        return val;
#elif defined(__x86_64__) || defined(_M_X64)
        return __rdtsc();
#else
        // Keep the fallback allocation/syscall-free as well. It is only a
        // monotonic work-unit counter on unsupported architectures; the
        // fixed reference frequency below makes the resulting telemetry
        // explicitly approximate rather than introducing a clock call into
        // an audio callback.
        static std::atomic<uint64_t> fallbackCounter{0};
        return fallbackCounter.fetch_add(1, std::memory_order_relaxed);
#endif
    }

    static uint64_t measureClockFrequency() {
#if defined(__arm64__) || defined(__aarch64__)
        uint64_t val;
        // Read counter frequency register on ARM64 (Apple Silicon)
        asm volatile("mrs %0, cntfrq_el0" : "=r" (val));
        return val;
#elif defined(__x86_64__) || defined(_M_X64)
        // Measure cycle count delta over 2 milliseconds at startup to calibrate frequency
        auto t0 = std::chrono::steady_clock::now();
        uint64_t c0 = __rdtsc();
        while (true) {
            auto t1 = std::chrono::steady_clock::now();
            auto dur = std::chrono::duration_cast<std::chrono::microseconds>(t1 - t0).count();
            if (dur >= 2000) {
                uint64_t c1 = __rdtsc();
                return (c1 - c0) * 500; // Cycles per second
            }
        }
#else
        return 1000000000; // 1 GHz reference fallback
#endif
    }
};

} // namespace Aura::Core
