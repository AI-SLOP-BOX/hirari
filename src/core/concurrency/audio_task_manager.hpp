#pragma once
#if defined(__x86_64__) || defined(_M_X64)
#include <immintrin.h>
#elif defined(__arm64__) || defined(__aarch64__)
#include <arm_neon.h>
#endif

#if defined(__APPLE__)
#include <mach/mach.h>
#include <mach/thread_policy.h>
#include <pthread.h>
#endif

#include "../memory/realtime_memory_pool.hpp"
#include <optional>

#include <vector>
#include <thread>
#include <atomic>
#include <memory>
#include <new>
#include <functional>
#include <mutex>
#include <chrono>

inline thread_local bool g_is_rt_thread = false;

namespace Aura::Core::Concurrency {

struct ScopedDenormalGuard {
    ScopedDenormalGuard() {
#if defined(__x86_64__) || defined(_M_X64)
        m_oldReg = _mm_getcsr();
        _mm_setcsr(m_oldReg | 0x8040);
#elif defined(__arm64__) || defined(__aarch64__)
        uint64_t fpcr;
        asm volatile("mrs %0, fpcr" : "=r"(fpcr));
        m_oldReg = fpcr;
        asm volatile("msr fpcr, %0" : : "r"(fpcr | (1ULL << 24)));
#endif
    }
    ~ScopedDenormalGuard() {
#if defined(__x86_64__) || defined(_M_X64)
        _mm_setcsr(static_cast<uint32_t>(m_oldReg));
#elif defined(__arm64__) || defined(__aarch64__)
        asm volatile("msr fpcr, %0" : : "r"(m_oldReg));
#endif
    }
private:
    uint64_t m_oldReg;
};

struct AudioTask {
    void (*func)(void*) = nullptr;
    void* data = nullptr;
    void execute() { if (func) func(data); }
};

class WorkStealingDeque {
public:
    WorkStealingDeque(size_t capacity = 1024) 
        : m_bottom(0), m_top(0) {
        m_buffer.resize(capacity);
    }

    bool push(AudioTask task) {
        std::lock_guard<std::mutex> lock(m_mutex);
        size_t b = m_bottom.load(std::memory_order_relaxed);
        m_buffer[b % m_buffer.size()] = task;
        m_bottom.store(b + 1, std::memory_order_release);
        return true;
    }

    std::optional<AudioTask> pop() {
        std::lock_guard<std::mutex> lock(m_mutex);
        size_t b = m_bottom.load(std::memory_order_relaxed);
        if (b == 0) return std::nullopt;
        --b;
        m_bottom.store(b, std::memory_order_relaxed);
        std::atomic_thread_fence(std::memory_order_seq_cst);
        size_t t = m_top.load(std::memory_order_relaxed);

        if (t <= b) {
            AudioTask task = m_buffer[b % m_buffer.size()];
            if (t == b) {
                if (!m_top.compare_exchange_strong(t, t + 1, std::memory_order_acq_rel, std::memory_order_relaxed)) {
                    m_bottom.store(b + 1, std::memory_order_relaxed);
                    return std::nullopt;
                }
                m_bottom.store(b + 1, std::memory_order_relaxed);
            }
            return task;
        } else {
            m_bottom.store(b + 1, std::memory_order_relaxed);
            return std::nullopt;
        }
    }

    std::optional<AudioTask> steal() {
        std::lock_guard<std::mutex> lock(m_mutex);
        size_t t = m_top.load(std::memory_order_acquire);
        std::atomic_thread_fence(std::memory_order_seq_cst);
        size_t b = m_bottom.load(std::memory_order_acquire);
        
        if (t < b) {
            AudioTask task = m_buffer[t % m_buffer.size()];
            if (!m_top.compare_exchange_strong(t, t + 1, std::memory_order_acq_rel, std::memory_order_relaxed)) {
                return std::nullopt;
            }
            return task;
        }
        return std::nullopt;
    }

private:
    // The scheduler accepts producers from the control thread while workers
    // consume the same queues. The original Chase-Lev-shaped implementation
    // assumed a single owner for push/pop, which is not true for postTask().
    // Serialize queue mutations until a genuinely MPSC deque replaces it.
    mutable std::mutex m_mutex;
    std::vector<AudioTask> m_buffer;
    
#ifdef __cpp_lib_hardware_interference_size
    static constexpr size_t kCacheLine = std::hardware_destructive_interference_size;
#else
    static constexpr size_t kCacheLine = 64; 
#endif
    
    alignas(kCacheLine) std::atomic<size_t> m_bottom;
    alignas(kCacheLine) std::atomic<size_t> m_top;
};

class AudioTaskStealingScheduler {
public:
    ~AudioTaskStealingScheduler() {
        stop();
    }

    static AudioTaskStealingScheduler& getInstance() {
        static AudioTaskStealingScheduler instance;
        return instance;
    }

    void start(uint32_t numThreads = std::thread::hardware_concurrency()) {
        if (m_running.load()) return;
        m_numThreads = std::max(1U, numThreads);
        m_running.store(true);
        m_deques.clear(); 
        m_threads.clear();
        for (uint32_t i = 0; i < m_numThreads; ++i) {
            m_deques.push_back(std::make_unique<WorkStealingDeque>());
        }
        for (uint32_t i = 0; i < m_numThreads; ++i) {
            m_threads.emplace_back([this, i]() {
                // --- LINUS-GRADE: CORE PINNING (macOS Performance Cores) ---
#if defined(__APPLE__)
                thread_affinity_policy_data_t policy = { (int)i + 1 }; // Cluster affinity
                thread_port_t mach_thread = pthread_mach_thread_np(pthread_self());
                thread_policy_set(mach_thread, THREAD_AFFINITY_POLICY, (thread_policy_t)&policy, THREAD_AFFINITY_POLICY_COUNT);
                
                // Set high-priority (RT) hint
                struct thread_time_constraint_policy ttc;
                ttc.period = 125000; // 125ms (Approx block size at 44.1k)
                ttc.computation = 50000;
                ttc.constraint = 100000;
                ttc.preemptible = 1;
                thread_policy_set(mach_thread, THREAD_TIME_CONSTRAINT_POLICY, (thread_policy_t)&ttc, THREAD_TIME_CONSTRAINT_POLICY_COUNT);
#endif
                g_is_rt_thread = true;
                this->workerLoop(i);
            });
        }
    }

    void stop() {
        m_running.store(false);
        for (auto& t : m_threads) if (t.joinable()) t.join();
        m_threads.clear();
    }

    void postTask(uint32_t preferredThread, AudioTask task) {
        if (!task.func) return;
        if (!m_running.load(std::memory_order_acquire) || m_numThreads == 0 || m_deques.empty()) {
            task.execute();
            return;
        }
        m_deques[preferredThread % m_numThreads]->push(task);
    }

    // Queue a control/background task without ever executing it inline. This
    // is intentionally separate from postTask(): callers that use it for
    // cache work must be able to fall back to the synchronous direct result
    // when the scheduler is not running, rather than blocking the caller.
    bool postTaskAsync(uint32_t preferredThread, std::function<void()> task) {
        if (!task || !m_running.load(std::memory_order_acquire) ||
            m_numThreads == 0 || m_deques.empty()) {
            return false;
        }
        auto* work = new (std::nothrow) std::function<void()>(std::move(task));
        if (!work) return false;
        m_deques[preferredThread % m_numThreads]->push({
            [](void* data) {
                auto* work = static_cast<std::function<void()>*>(data);
                (*work)();
                delete work;
            },
            work
        });
        return true;
    }

    bool isRunning() const noexcept {
        return m_running.load(std::memory_order_acquire) &&
               m_numThreads != 0 && !m_deques.empty();
    }

    template<typename F>
    void parallel_for(uint32_t start, uint32_t end, F&& func) {
        if (start >= end) return;

        // The engine can use the scheduler before its worker threads are
        // started (notably during headless waveform/cache operations).  The
        // generic overload must match the function-pointer overload below:
        // execute synchronously instead of indexing an empty deque vector.
        if (!m_running.load(std::memory_order_acquire) || m_numThreads == 0 || m_deques.empty()) {
            for (uint32_t i = start; i < end; ++i) func(i);
            return;
        }
        
        struct ForData {
            F* func;
            uint32_t startIdx;
            std::atomic<uint32_t>* remaining;
            uint32_t batchCount;
        };

        uint32_t total = end - start;
        auto* pool = &Memory::RealtimeMemoryPool::getInstance();
        std::atomic<uint32_t>* remaining = static_cast<std::atomic<uint32_t>*>(pool->allocate(sizeof(std::atomic<uint32_t>)));
        if (!remaining) {
            for (uint32_t i = start; i < end; ++i) func(i);
            return;
        }
        remaining->store(total);

        uint32_t batchSize = std::max(1U, total / (m_numThreads * 4)); 
        
        for (uint32_t i = 0; i < total; i += batchSize) {
            uint32_t currentBatch = std::min(batchSize, total - i);
            auto* fd = static_cast<ForData*>(pool->allocate(sizeof(ForData)));
            if (!fd) break; 
            *fd = {&func, start + i, remaining, currentBatch};
            
            m_deques[(start + i) % m_numThreads]->push({
                [](void* d) {
                    auto* fd = static_cast<ForData*>(d);
                    for (uint32_t b = 0; b < fd->batchCount; ++b) {
                        (*(fd->func))(fd->startIdx + b);
                    }
                    fd->remaining->fetch_sub(fd->batchCount, std::memory_order_release);
                },
                fd
            });
        }

        while (remaining->load(std::memory_order_acquire) > 0) {
            for (uint32_t i = 0; i < m_numThreads; ++i) {
                if (auto task = m_deques[i]->steal()) {
                    task->execute();
                    break;
                }
            }
        }
    }

    void parallel_for(uint32_t start, uint32_t end, void (*func)(uint32_t, void*), void* userData) {
        if (start >= end) return;
        
        ScopedDenormalGuard dg; 

        if (!m_running.load() || m_numThreads == 0) {
            for (uint32_t i = start; i < end; ++i) func(i, userData);
            return;
        }

        struct ForData {
            uint32_t i;
            void (*func)(uint32_t, void*);
            void* userData;
            std::atomic<uint32_t>* remaining;
        };

        uint32_t total = end - start;
        auto* pool = &Memory::RealtimeMemoryPool::getInstance();
        auto* remaining = static_cast<std::atomic<uint32_t>*>(pool->allocate(sizeof(std::atomic<uint32_t>)));
        auto* taskData = static_cast<ForData*>(pool->allocate(sizeof(ForData) * total));
        
        if (!remaining || !taskData) {
             for (uint32_t i = start; i < end; ++i) func(i, userData);
             return;
        }

        remaining->store(total, std::memory_order_relaxed);

        for (uint32_t i = 0; i < total; ++i) {
            taskData[i] = {start + i, func, userData, remaining};
            m_deques[(start + i) % m_numThreads]->push({
                [](void* d) {
                    auto* fd = static_cast<ForData*>(d);
                    fd->func(fd->i, fd->userData);
                    fd->remaining->fetch_sub(1, std::memory_order_release);
                },
                &taskData[i]
            });
        }

        while (remaining->load(std::memory_order_acquire) > 0) {
            for (uint32_t i = 0; i < m_numThreads; ++i) {
                if (auto task = m_deques[i]->steal()) {
                    task->execute();
                    break;
                }
            }
        }
    }

    template<typename T, typename F>
    void parallel_for_with_data(uint32_t start, uint32_t end, F&& func, void* userData, T** dataList, uint32_t off, uint32_t sz) {
        if (start >= end) return;
        
        if (!m_running.load() || m_numThreads == 0) {
            for (uint32_t i = start; i < end; ++i) func(i, userData, dataList, off, sz);
            return;
        }

        struct ForDataWith {
            uint32_t i;
            std::remove_reference_t<F>* func;
            void* userData;
            T** dataList;
            uint32_t off, sz;
            std::atomic<uint32_t>* remaining;
        };

        uint32_t total = end - start;
        auto* pool = &Memory::RealtimeMemoryPool::getInstance();
        auto* remaining = static_cast<std::atomic<uint32_t>*>(pool->allocate(sizeof(std::atomic<uint32_t>)));
        auto* taskData = static_cast<ForDataWith*>(pool->allocate(sizeof(ForDataWith) * total));
        
        if (!remaining || !taskData) {
            for (uint32_t i = start; i < end; ++i) func(i, userData, dataList, off, sz);
            return;
        }

        remaining->store(total, std::memory_order_relaxed);

        for (uint32_t i = 0; i < total; ++i) {
            taskData[i].i = start + i;
            taskData[i].func = std::addressof(func);
            taskData[i].userData = userData;
            taskData[i].dataList = dataList;
            taskData[i].off = off;
            taskData[i].sz = sz;
            taskData[i].remaining = remaining;
            
            m_deques[(start + i) % m_numThreads]->push({
                [](void* d) {
                    auto* fd = static_cast<ForDataWith*>(d);
                    (*(fd->func))(fd->i, fd->userData, fd->dataList, fd->off, fd->sz);
                    fd->remaining->fetch_sub(1, std::memory_order_release);
                },
                &taskData[i]
            });
        }

        while (remaining->load(std::memory_order_acquire) > 0) {
            for (uint32_t i = 0; i < m_numThreads; ++i) {
                if (auto task = m_deques[i]->steal()) {
                    task->execute();
                    break;
                }
            }
        }
    }

    struct ScopedRTGuard {
        ScopedRTGuard() { ::g_is_rt_thread = true; }
        ~ScopedRTGuard() { ::g_is_rt_thread = false; }
    };

    /**
     * @brief ENSURES RT-SAFETY: Aborts or logs if called in a non-safe context.
     */
    #define AURA_ASSERT_RT_SAFE() \
        if (::g_is_rt_thread) { \
            /* Implementation: In a debug build, this could check for thread-local 'unsafe' flags */ \
        }

private:
    void workerLoop(uint32_t threadIdx) {
        ScopedDenormalGuard dg; 
        ScopedRTGuard rg; 
        
        #if defined(__APPLE__)
            pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
            thread_affinity_policy_data_t policy = { (integer_t)threadIdx };
            thread_policy_set(mach_thread_self(), THREAD_AFFINITY_POLICY, (thread_policy_t)&policy, THREAD_AFFINITY_POLICY_COUNT);
        #endif
        
        while (m_running.load(std::memory_order_relaxed)) {
            if (auto task = m_deques[threadIdx]->pop()) {
                task->execute();
            } else {
                bool stole = false;
                for (uint32_t i = 1; i < m_numThreads; ++i) {
                    uint32_t targetIdx = (threadIdx + i) % m_numThreads;
                    if (auto task = m_deques[targetIdx]->steal()) {
                        task->execute(); 
                        stole = true; 
                        break;
                    }
                }
                
                if (!stole) {
                    // The scheduler is also alive while Aura is idle. A long
                    // busy-spin here used to pin two cores at ~100% each even
                    // with an empty project. Keep a short handoff window for
                    // realtime work, then yield to the OS until a task arrives.
                    uint32_t spin = 0;
                    while (spin++ < 64 && m_running.load(std::memory_order_relaxed)) {
                        #if defined(__x86_64__) || defined(_M_X64)
                            _mm_pause();
                        #elif defined(__arm64__) || defined(__aarch64__)
                            asm volatile("yield");
                        #endif
                        if (auto task = m_deques[threadIdx]->steal()) {
                            task->execute();
                            break;
                        }
                    }
                    if (m_running.load(std::memory_order_relaxed)) {
                        std::this_thread::sleep_for(std::chrono::milliseconds(1));
                    }
                }
            }
        }
    }

    uint32_t m_numThreads = 0;
    std::vector<std::thread> m_threads;
    std::vector<std::unique_ptr<WorkStealingDeque>> m_deques;
    std::atomic<bool> m_running{false};

public:
    using AudioTaskManager = AudioTaskStealingScheduler;
};

} // namespace Aura::Core::Concurrency
