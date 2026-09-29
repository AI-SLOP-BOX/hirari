#pragma once
#include <atomic>
#include <vector>
#include <optional>

namespace Hirari::Core::Sync {

/**
 * @class LockFreeQueue
 * @brief High-performance Single-Producer Single-Consumer (SPSC) queue.
 * ESSENTIAL for jitter-free UI-to-DSP parameter updates.
 */
template<typename T, size_t Size>
class LockFreeQueue {
public:
    static_assert((Size & (Size - 1)) == 0, "Size must be a power of 2");

    LockFreeQueue() : m_head(0), m_tail(0) {}

    bool push(const T& val) {
        size_t head = m_head.load(std::memory_order_relaxed);
        size_t nextHead = (head + 1) & (Size - 1);
        if (nextHead == m_tail.load(std::memory_order_acquire)) return false; // Full
        m_buffer[head] = val;
        m_head.store(nextHead, std::memory_order_release);
        return true;
    }

    std::optional<T> pop() {
        size_t tail = m_tail.load(std::memory_order_relaxed);
        if (tail == m_head.load(std::memory_order_acquire)) return std::nullopt; // Empty
        T val = m_buffer[tail];
        m_tail.store((tail + 1) & (Size - 1), std::memory_order_release);
        return val;
    }

private:
    static constexpr size_t kCacheLineSize = 64;
    alignas(kCacheLineSize) std::atomic<size_t> m_head;
    alignas(kCacheLineSize) std::atomic<size_t> m_tail;
    T m_buffer[Size];
};

} // namespace Hirari::Core::Sync
