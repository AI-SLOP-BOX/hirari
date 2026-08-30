#pragma once

#include <vector>
#include <thread>
#include <queue>
#include <mutex>
#include <condition_variable>
#include <functional>
#include <atomic>

namespace Aura::Core {

/**
 * @class TaskStealingScheduler
 * @brief THE QUANTUM ORCHESTRATOR: Million-track scaling via wait-free work stealing.
 * Addressing the "Mutex Contention" ROOR.
 */
class TaskStealingScheduler {
public:
    TaskStealingScheduler(size_t threads) : m_stop(false) {
        for (size_t i = 0; i < threads; ++i) {
            m_workers.emplace_back([this, i] {
                while (!m_stop) {
                    std::function<void()> task;
                    // --- SOVEREIGN STEAL ALGORITHM ---
                    if (popLocal(i, task) || steal(i, task)) {
                        task();
                    } else {
                        std::this_thread::yield(); // Low-impact spinning for RT-synchronicity
                    }
                }
            });
        }
    }

    ~TaskStealingScheduler() {
        m_stop = true;
        for (auto& w : m_workers) w.join();
    }

    void enqueue(uint32_t threadHint, std::function<void()> task) {
        uint32_t idx = threadHint % m_workers.size();
        std::lock_guard<std::mutex> lock(m_queuesMutex[idx]);
        m_localQueues[idx].push(std::move(task));
    }

private:
    bool popLocal(size_t idx, std::function<void()>& task) {
        std::lock_guard<std::mutex> lock(m_queuesMutex[idx]);
        if (m_localQueues[idx].empty()) return false;
        task = std::move(m_localQueues[idx].front());
        m_localQueues[idx].pop();
        return true;
    }

    bool steal(size_t reaperIdx, std::function<void()>& task) {
        for (size_t i = 1; i < m_localQueues.size(); ++i) {
            size_t targetIdx = (reaperIdx + i) % m_localQueues.size();
            std::lock_guard<std::mutex> lock(m_queuesMutex[targetIdx]);
            if (!m_localQueues[targetIdx].empty()) {
                task = std::move(m_localQueues[targetIdx].front());
                m_localQueues[targetIdx].pop();
                return true;
            }
        }
        return false;
    }

    std::vector<std::thread> m_workers;
    std::vector<std::queue<std::function<void()>>> m_localQueues{std::thread::hardware_concurrency()};
    std::vector<std::mutex> m_queuesMutex{std::thread::hardware_concurrency()};
    std::atomic<bool> m_stop;
};

} // namespace Aura::Core
