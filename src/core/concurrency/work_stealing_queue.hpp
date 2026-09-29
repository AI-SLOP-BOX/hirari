#pragma once

#include <vector>
#include <atomic>
#include <memory>
#include <deque>
#include <mutex>
#include <thread>
#include <new>

namespace Hirari::Concurrency {

/**
 * @class WorkStealingQueue
 * @brief Professional Work-Stealing Task Queue.
 * HONEST FIX: Refactored Task to be a fixed-size trivially-copyable struct.
 * This guarantees ZERO heap allocations during task pushing/stealing.
 */
class WorkStealingQueue {
public:
    static constexpr size_t kCapacity = 4096;
    static constexpr size_t kMask = kCapacity - 1;

    /**
     * @brief Heap-free Task structure with inline storage for functors.
     */
    struct Task {
        typedef void (*RunPtr)(void*);
        RunPtr run = nullptr;
        alignas(16) char storage[112]; // Total Task size matches 128 bytes (2 cache lines)

        template <typename F>
        static Task create(F&& f) {
            static_assert(sizeof(F) <= sizeof(storage), "Functor too large for Task inline storage");
            Task t;
            new (t.storage) F(std::forward<F>(f));
            t.run = [](void* ptr) {
                F* fn = static_cast<F*>(ptr);
                (*fn)();
                fn->~F();
            };
            return t;
        }

        void operator()() {
            if (run) run(storage);
        }
    };

    WorkStealingQueue(uint32_t numThreads) : m_numThreads(numThreads) {
        m_queues.reserve(numThreads);
        for(uint32_t i = 0; i < numThreads; ++i) {
            auto q = std::make_unique<Deque>();
            q->top.store(0, std::memory_order_relaxed);
            q->bottom.store(0, std::memory_order_relaxed);
            q->buffer.resize(kCapacity);
            m_queues.push_back(std::move(q));
        }
    }

    /**
     * @brief PUSH: Thread adds task to its own queue (LIFO).
     */
    void push(uint32_t threadId, Task t) {
        auto& q = *m_queues[threadId];
        int64_t b = q.bottom.load(std::memory_order_relaxed);
        q.buffer[b & kMask] = t;
        // Ensure task is written before bottom is visible
        q.bottom.store(b + 1, std::memory_order_release);
    }

    /**
     * @brief POP: Thread pulls task from its own queue (LIFO).
     */
    bool pop(uint32_t threadId, Task& t) {
        auto& q = *m_queues[threadId];
        int64_t b = q.bottom.load(std::memory_order_relaxed) - 1;
        q.bottom.store(b, std::memory_order_seq_cst);
        int64_t t_idx = q.top.load(std::memory_order_seq_cst);
        
        if (t_idx <= b) {
            t = q.buffer[b & kMask];
            if (t_idx != b) return true;
            
            // Last element: compete with stealers
            int64_t expected_t = t_idx;
            if (!q.top.compare_exchange_strong(expected_t, t_idx + 1, std::memory_order_seq_cst)) {
                q.bottom.store(b + 1, std::memory_order_relaxed);
                return false;
            }
            q.bottom.store(b + 1, std::memory_order_relaxed);
            return true;
        }
        q.bottom.store(b + 1, std::memory_order_relaxed);
        return false;
    }

    /**
     * @brief STEAL: One thread steals from another's TOP (FIFO).
     */
    bool steal(uint32_t threadId, Task& t) {
        for (uint32_t i = 1; i < m_numThreads; ++i) {
            uint32_t victim = (threadId + i) % m_numThreads;
            auto& q = *m_queues[victim];
            
            int64_t t_idx = q.top.load(std::memory_order_acquire);
            int64_t b = q.bottom.load(std::memory_order_acquire);
            
            if (t_idx < b) {
                // Must read task BEFORE CAS to avoid race with pop
                Task stolenTask = q.buffer[t_idx & kMask]; 
                
                if (!q.top.compare_exchange_strong(t_idx, t_idx + 1, std::memory_order_seq_cst)) {
                    continue; // Lost competition
                }
                t = stolenTask;
                return true;
            }
        }
        return false;
    }

private:
    struct Deque {
        alignas(64) std::atomic<int64_t> top;
        alignas(64) std::atomic<int64_t> bottom;
        std::vector<Task> buffer;
    };
    uint32_t m_numThreads;
    std::vector<std::unique_ptr<Deque>> m_queues;
};

} // namespace Hirari::Concurrency
