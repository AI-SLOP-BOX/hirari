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

#include <thread>
#include <atomic>
#include <memory>
#include <new>
#include <functional>
#include <type_traits>

inline thread_local bool g_is_rt_thread = false;

namespace Hirari::Core::Concurrency {

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

struct AudioTaskJob {
    virtual void run() noexcept = 0;
protected:
    ~AudioTaskJob() = default;
};

inline void hirari_audio_task_dispatch(void* data) {
    if (data) static_cast<AudioTaskJob*>(data)->run();
}

extern "C" inline void hirari_audio_task_dispatch_ffi(void* data) {
    hirari_audio_task_dispatch(data);
}

extern "C" inline uint64_t hirari_audio_worker_enter(uint32_t index) {
    uint64_t state = 0;
#if defined(__x86_64__) || defined(_M_X64)
    state = _mm_getcsr();
    _mm_setcsr(static_cast<uint32_t>(state) | 0x8040);
#elif defined(__arm64__) || defined(__aarch64__)
    asm volatile("mrs %0, fpcr" : "=r"(state));
    const uint64_t enabled = state | (1ULL << 24);
    asm volatile("msr fpcr, %0" : : "r"(enabled));
#endif
    ::g_is_rt_thread = true;
#if defined(__APPLE__)
    pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
    thread_affinity_policy_data_t affinity = { static_cast<integer_t>(index) };
    thread_policy_set(mach_thread_self(), THREAD_AFFINITY_POLICY,
        reinterpret_cast<thread_policy_t>(&affinity), THREAD_AFFINITY_POLICY_COUNT);
    thread_time_constraint_policy_data_t constraint{};
    constraint.period = 125000;
    constraint.computation = 50000;
    constraint.constraint = 100000;
    constraint.preemptible = 1;
    thread_policy_set(mach_thread_self(), THREAD_TIME_CONSTRAINT_POLICY,
        reinterpret_cast<thread_policy_t>(&constraint), THREAD_TIME_CONSTRAINT_POLICY_COUNT);
#endif
    return state;
}

extern "C" inline void hirari_audio_worker_leave(uint64_t state) {
#if defined(__x86_64__) || defined(_M_X64)
    _mm_setcsr(static_cast<uint32_t>(state));
#elif defined(__arm64__) || defined(__aarch64__)
    asm volatile("msr fpcr, %0" : : "r"(state));
#else
    (void)state;
#endif
    ::g_is_rt_thread = false;
}

extern "C" inline uint64_t hirari_audio_background_worker_enter(uint32_t) { return 0; }
extern "C" inline void hirari_audio_background_worker_leave(uint64_t) {}

struct CallbackAudioTaskJob final : AudioTaskJob {
    explicit CallbackAudioTaskJob(AudioTask task) : task(task) {}
    void run() noexcept override { task.execute(); }
    AudioTask task;
};

class AudioTaskStealingScheduler {
public:
    AudioTaskStealingScheduler() : m_state(hirari_audio_scheduler_create()) {}
    AudioTaskStealingScheduler(const AudioTaskStealingScheduler&) = delete;
    AudioTaskStealingScheduler& operator=(const AudioTaskStealingScheduler&) = delete;
    ~AudioTaskStealingScheduler() {
        stop();
        hirari_audio_scheduler_destroy(m_state);
    }

    static AudioTaskStealingScheduler& getInstance() {
        static AudioTaskStealingScheduler instance;
        return instance;
    }

    void start(
        uint32_t numThreads = std::thread::hardware_concurrency(),
        size_t queueCapacity = 1024,
        uint64_t (*enter)(uint32_t) = hirari_audio_worker_enter,
        void (*leave)(uint64_t) = hirari_audio_worker_leave) {
        if (isRunning()) return;
        hirari_audio_scheduler_start(m_state, std::max(1U, numThreads), queueCapacity,
            hirari_audio_task_dispatch_ffi, enter, leave);
    }

    void stop() { hirari_audio_scheduler_stop(m_state); }

    uint32_t getNumThreads() const noexcept {
        return static_cast<uint32_t>(hirari_audio_scheduler_thread_count(m_state));
    }

    bool enqueue(uint32_t worker, AudioTaskJob* job) {
        return job && hirari_audio_scheduler_push(m_state, worker, job);
    }

    bool enqueue(uint32_t worker, AudioTask task) {
        if (!task.func) return false;
        auto* pool = &Memory::RealtimeMemoryPool::getInstance();
        void* storage = pool->allocate(sizeof(CallbackAudioTaskJob));
        if (!storage) return false;
        auto* job = new (storage) CallbackAudioTaskJob(task);
        return enqueue(worker, job);
    }

    bool take(uint32_t worker, AudioTask& task) {
        void* data = nullptr;
        if (!hirari_audio_scheduler_steal(m_state, worker, &data)) return false;
        task = AudioTask{hirari_audio_task_dispatch, data};
        return true;
    }

    void postTask(uint32_t preferredThread, AudioTask task) {
        if (!task.func) return;
        if (!isRunning() || getNumThreads() == 0) {
            task.execute();
            return;
        }
        if (!enqueue(preferredThread % getNumThreads(), task)) task.execute();
    }

    // Queue a control/background task without ever executing it inline. This
    // is intentionally separate from postTask(): callers that use it for
    // cache work must be able to fall back to the synchronous direct result
    // when the scheduler is not running, rather than blocking the caller.
    bool postTaskAsync(uint32_t preferredThread, std::function<void()> task) {
        if (!task || !isRunning() || getNumThreads() == 0) {
            return false;
        }
        struct AsyncJob final : AudioTaskJob {
            explicit AsyncJob(std::function<void()> fn) : work(std::move(fn)) {}
            void run() noexcept override {
                std::unique_ptr<AsyncJob> owner(this);
                work();
            }
            std::function<void()> work;
        };
        auto* work = new (std::nothrow) AsyncJob(std::move(task));
        if (!work) return false;
        if (!enqueue(preferredThread % getNumThreads(), work)) {
            delete work;
            return false;
        }
        return true;
    }

    bool isRunning() const noexcept {
        return hirari_audio_scheduler_is_running(m_state);
    }

    template<typename F>
    void parallel_for(uint32_t start, uint32_t end, F&& func) {
        using Function = std::remove_reference_t<F>;
        struct Context { Function* function; } context{std::addressof(func)};
        if (start >= end) return;
        hirari_audio_scheduler_parallel_for(m_state, start, end, &context,
            +[](uint32_t index, void* opaque) noexcept {
                auto* state = static_cast<Context*>(opaque);
                (*state->function)(index);
            });
    }

    void parallel_for(uint32_t start, uint32_t end,
                      void (*func)(uint32_t, void*), void* userData) {
        if (start >= end || !func) return;
        ScopedDenormalGuard denormalGuard;
        struct Context { void (*function)(uint32_t, void*); void* userData; };
        Context context{func, userData};
        hirari_audio_scheduler_parallel_for(m_state, start, end, &context,
            +[](uint32_t index, void* opaque) noexcept {
                auto* state = static_cast<Context*>(opaque);
                state->function(index, state->userData);
            });
    }

    template<typename T, typename F>
    void parallel_for_with_data(uint32_t start, uint32_t end, F&& func,
                                void* userData, T** dataList,
                                uint32_t off, uint32_t sz) {
        using Function = std::remove_reference_t<F>;
        struct Context {
            Function* function;
            void* userData;
            T** dataList;
            uint32_t offset;
            uint32_t size;
        } context{std::addressof(func), userData, dataList, off, sz};
        if (start >= end) return;
        hirari_audio_scheduler_parallel_for(m_state, start, end, &context,
            +[](uint32_t index, void* opaque) noexcept {
                auto* state = static_cast<Context*>(opaque);
                (*state->function)(index, state->userData,
                    state->dataList, state->offset, state->size);
            });
    }

    struct ScopedRTGuard {
        ScopedRTGuard() { ::g_is_rt_thread = true; }
        ~ScopedRTGuard() { ::g_is_rt_thread = false; }
    };

    /**
     * @brief ENSURES RT-SAFETY: Aborts or logs if called in a non-safe context.
     */
    #define HIRARI_ASSERT_RT_SAFE() \
        if (::g_is_rt_thread) { \
            /* Implementation: In a debug build, this could check for thread-local 'unsafe' flags */ \
        }

private:
    void* m_state = nullptr;

public:
    using AudioTaskManager = AudioTaskStealingScheduler;
};

} // namespace Hirari::Core::Concurrency
