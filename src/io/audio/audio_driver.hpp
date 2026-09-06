#pragma once
#include <thread>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <vector>
#include <algorithm>
#include <iostream>
#include <chrono>
#include <condition_variable>
#include <mutex>
#ifdef __APPLE__
#include <mach/mach_init.h>
#include <mach/thread_policy.h>
#include <mach/thread_act.h>
#endif

namespace Aura::IO::Audio {

/**
 * @class RealtimeAudioDriver
 * @brief High-Priority Real-time Audio Engine Driver.
 */
class RealtimeAudioDriver {
public:
    ~RealtimeAudioDriver() { stop(); }

    bool start(::Aura::AuraEngine& engine, double sr, uint32_t bs) {
        if (!std::isfinite(sr) || sr < 8000.0 || sr > 384000.0 || bs == 0 || bs > 8192 ||
            m_running.exchange(true, std::memory_order_acq_rel)) {
            return false;
        }
        try {
            m_audioThread = std::thread([this, &engine, sr, bs]() {
            #ifdef __APPLE__
            thread_time_constraint_policy_data_t policy;
            policy.period = (uint32_t)(1e9 * bs / sr);
            policy.computation = (uint32_t)(policy.period * 0.85);
            policy.constraint = policy.computation;
            policy.preemptible = 1;
            thread_policy_set(mach_thread_self(), THREAD_TIME_CONSTRAINT_POLICY, 
                             (thread_policy_t)&policy, THREAD_TIME_CONSTRAINT_POLICY_COUNT);
            #endif

            std::vector<float> L(bs), R(bs);
            auto next = std::chrono::steady_clock::now();
            auto dur = std::chrono::nanoseconds((long)(1e9 * bs / sr));

            while (m_running.load(std::memory_order_acquire)) {
                try {
                    std::fill(L.begin(), L.end(), 0.0f);
                    std::fill(R.begin(), R.end(), 0.0f);
                    engine.process(L.data(), R.data(), bs);
                    for (uint32_t i = 0; i < bs; ++i) {
                        if (!std::isfinite(L[i])) L[i] = 0.0f;
                        if (!std::isfinite(R[i])) R[i] = 0.0f;
                    }
                } catch (...) {
                    // Never allow an exception to cross the realtime thread
                    // boundary. Emit one block of silence and keep the device
                    // alive so the UI can report the fault.
                    std::fill(L.begin(), L.end(), 0.0f);
                    std::fill(R.begin(), R.end(), 0.0f);
                    m_callbackFaults.fetch_add(1, std::memory_order_relaxed);
                }
                next += dur;
                std::unique_lock<std::mutex> lock(m_waitMutex);
                m_wait.wait_until(lock, next, [this] {
                    return !m_running.load(std::memory_order_acquire);
                });
            }
            });
        } catch (...) {
            m_running.store(false, std::memory_order_release);
            return false;
        }
        return true;
    }

    void stop() {
        m_running.store(false, std::memory_order_release);
        m_wait.notify_all();
        if (m_audioThread.joinable()) m_audioThread.join();
    }

private:
    std::atomic<bool> m_running{false};
    std::atomic<uint64_t> m_callbackFaults{0};
    std::thread m_audioThread;
    std::condition_variable m_wait;
    std::mutex m_waitMutex;
};

} // namespace Aura::IO::Audio
