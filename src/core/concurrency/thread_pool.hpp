#pragma once
#include <vector>
#include <queue>
#include <thread>
#include <mutex>
#include <condition_variable>
#include <functional>
#include <future>
#include <type_traits>
#include <algorithm>

namespace Aura::Core::Concurrency {

/**
 * @class ThreadPool
 * @brief Professional Task-based Concurrency Engine.
 * HONEST FIX: Replaces dangerous 'std::async' thread explosion with 
 * a core-count-limited worker pool. Prevents CPU context-switch thrashing.
 */
class ThreadPool {
public:
    static ThreadPool& getInstance() {
        const auto detected = std::thread::hardware_concurrency();
        // The standard permits hardware_concurrency() to return 0 when the
        // platform cannot report a value.  A zero-sized pool would accept
        // work forever without a worker to execute it.
        static ThreadPool instance(detected == 0 ? 1u : detected);
        return instance;
    }

    template<class F, class... Args>
    auto enqueue(F&& f, Args&&... args)
        -> std::future<std::invoke_result_t<F, Args...>> {
        using return_type = std::invoke_result_t<F, Args...>;

        auto task = std::make_shared<std::packaged_task<return_type()>>(
            std::bind(std::forward<F>(f), std::forward<Args>(args)...)
        );
        
        std::future<return_type> res = task->get_future();
        {
            std::unique_lock<std::mutex> lock(m_queueMutex);
            if (m_stop) throw std::runtime_error("ThreadPool stopped");
            m_tasks.emplace([task](){ (*task)(); });
        }
        m_condition.notify_one();
        return res;
    }

    ~ThreadPool() {
        {
            std::unique_lock<std::mutex> lock(m_queueMutex);
            m_stop = true;
        }
        m_condition.notify_all();
        for (std::thread& worker : m_workers) worker.join();
    }

private:
    explicit ThreadPool(size_t threads) : m_stop(false) {
        threads = std::max<size_t>(1u, threads);
        for (size_t i = 0; i < threads; ++i) {
            m_workers.emplace_back([this] {
                for (;;) {
                    std::function<void()> task;
                    {
                        std::unique_lock<std::mutex> lock(this->m_queueMutex);
                        this->m_condition.wait(lock, [this]{ return this->m_stop || !this->m_tasks.empty(); });
                        if (this->m_stop && this->m_tasks.empty()) return;
                        task = std::move(this->m_tasks.front());
                        this->m_tasks.pop();
                    }
                    task();
                }
            });
        }
    }

    std::vector<std::thread> m_workers;
    std::queue<std::function<void()>> m_tasks;
    std::mutex m_queueMutex;
    std::condition_variable m_condition;
    bool m_stop;
};

} // namespace Aura::Core::Concurrency
