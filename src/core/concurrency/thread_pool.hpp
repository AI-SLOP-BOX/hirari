#pragma once

#include "audio_task_manager.hpp"

#include <algorithm>
#include <functional>
#include <future>
#include <memory>
#include <stdexcept>
#include <thread>
#include <type_traits>
#include <utility>

namespace Hirari::Core::Concurrency {

/** Task-based background pool backed by the Rust-owned worker scheduler. */
class ThreadPool {
    struct FutureTask final : AudioTaskJob {
        explicit FutureTask(std::function<void()> callback) : work(std::move(callback)) {}

        void run() noexcept override {
            std::unique_ptr<FutureTask> owner(this);
            work();
        }

        std::function<void()> work;
    };

public:
    static ThreadPool& getInstance() {
        const auto detected = std::thread::hardware_concurrency();
        static ThreadPool instance(detected == 0 ? 1u : detected);
        return instance;
    }

    explicit ThreadPool(size_t threads) {
        threads = std::max<size_t>(1, threads);
        m_scheduler.start(static_cast<uint32_t>(threads), 8192,
            hirari_audio_background_worker_enter, hirari_audio_background_worker_leave);
        if (!m_scheduler.isRunning()) {
            throw std::runtime_error("Failed to start Rust background task scheduler");
        }
    }

    ThreadPool(const ThreadPool&) = delete;
    ThreadPool& operator=(const ThreadPool&) = delete;

    template<class F, class... Args>
    auto enqueue(F&& f, Args&&... args)
        -> std::future<std::invoke_result_t<F, Args...>> {
        using return_type = std::invoke_result_t<F, Args...>;
        auto task = std::make_shared<std::packaged_task<return_type()>>(
            std::bind(std::forward<F>(f), std::forward<Args>(args)...));
        std::future<return_type> result = task->get_future();
        auto* job = new FutureTask([task] { (*task)(); });
        const auto worker = m_nextWorker.fetch_add(1, std::memory_order_relaxed);
        if (!m_scheduler.enqueue(worker, job)) {
            delete job;
            throw std::runtime_error("Rust background task scheduler is stopped or full");
        }
        return result;
    }

    ~ThreadPool() { m_scheduler.stop(); }

private:
    AudioTaskStealingScheduler m_scheduler;
    std::atomic<uint32_t> m_nextWorker{0};
};

} // namespace Hirari::Core::Concurrency
