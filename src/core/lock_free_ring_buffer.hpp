#pragma once

#include <vector>
#include <atomic>
#include <memory>

namespace Aura::Core {

/**
 * @brief LockFreeRingBuffer: Fast SPSC (Single Producer, Single Consumer) queue.
 * Transmits spectral data or parameter logs from Audio to UI without Mutex stalls.
 */
template <typename T, size_t Size>
class LockFreeRingBuffer {
public:
    static_assert((Size & (Size - 1)) == 0, "Size must be power of 2 for bitmask efficiency");

    LockFreeRingBuffer() : m_writeIdx(0), m_readIdx(0) {
        m_buffer = std::make_unique<T[]>(Size);
    }

    /**
     * @brief Pushes a sample to the buffer from the Audio thread.
     */
    bool push(T sample) {
        size_t writeIdx = m_writeIdx.load(std::memory_order_relaxed);
        size_t nextWriteIdx = (writeIdx + 1) & (Size - 1);
        if (nextWriteIdx == m_readIdx.load(std::memory_order_acquire)) return false; // Full
        m_buffer[writeIdx] = sample;
        m_writeIdx.store(nextWriteIdx, std::memory_order_release);
        return true;
    }

    /**
     * @brief Pops a sample from the buffer for the UI thread.
     */
    bool pop(T& out) {
        size_t readIdx = m_readIdx.load(std::memory_order_relaxed);
        if (readIdx == m_writeIdx.load(std::memory_order_acquire)) return false; // Empty
        out = m_buffer[readIdx];
        m_readIdx.store((readIdx + 1) & (Size - 1), std::memory_order_release);
        return true;
    }

    // Control-thread lifecycle operation. Call only after the consumer has
    // stopped; resetting indices while push/pop are active would violate the
    // SPSC ownership contract. This is used to prevent frames left behind by
    // a failed recording session from entering the next session.
    void reset() noexcept {
        m_readIdx.store(0, std::memory_order_relaxed);
        m_writeIdx.store(0, std::memory_order_relaxed);
    }

private:
    std::unique_ptr<T[]> m_buffer;
    alignas(64) std::atomic<size_t> m_writeIdx;
    alignas(64) std::atomic<size_t> m_readIdx;
};

} // namespace Aura::Core
