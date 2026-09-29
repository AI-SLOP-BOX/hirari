#pragma once

#include <vector>
#include <thread>
#include <queue>
#include <mutex>
#include <condition_variable>
#include <functional>
#include <atomic>

namespace Hirari::Core {

/**
 * @brief WorkerThreadPool: Parallel DSP processing for large-scale multi-voice instruments.
 * Addresses the "serial rendering bottleneck" from the review.
 */
class WorkerThreadPool {
public:
    WorkerThreadPool(size_t numThreads = std::thread::hardware_concurrency()) 
        : m_stop(false) {
        for (size_t i = 0; i < numThreads; ++i) {
            m_workers.emplace_back([this] {
                while (true) {
                    std::function<void()> task;
                    {
                        std::unique_lock<std::mutex> lock(m_queueMutex);
                        m_condition.wait(lock, [this] { return m_stop || !m_tasks.empty(); });
                        if (m_stop && m_tasks.empty()) return;
                        task = std::move(m_tasks.front());
                        m_tasks.pop();
                    }
                    task();
                    m_completedTasks++;
                }
            });
        }
    }

    ~WorkerThreadPool() {
        { std::unique_lock<std::mutex> lock(m_queueMutex); m_stop = true; }
        m_condition.notify_all();
        for (std::thread& worker : m_workers) worker.join();
    }

    /**
     * @brief Enqueues a DSP task (rendering a voice/track) to be processed in parallel.
     */
    void enqueue(std::function<void()> task) {
        { std::unique_lock<std::mutex> lock(m_queueMutex); m_tasks.push(std::move(task)); }
        m_condition.notify_one();
    }

    void waitForCompletion(size_t expected) {
        while (m_completedTasks.load() < expected) std::this_thread::yield();
        m_completedTasks.store(0);
    }

private:
    std::vector<std::thread> m_workers;
    std::queue<std::function<void()>> m_tasks;
    std::mutex m_queueMutex;
    std::condition_variable m_condition;
    std::atomic<bool> m_stop;
    std::atomic<size_t> m_completedTasks{0};
};

} // namespace Hirari::Core
