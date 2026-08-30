#pragma once
#include <atomic>
#include <cstdint>

namespace Aura::Core {

/**
 * @struct RingBuffer
 * @brief Industrial-grade, Lock-free Single-Producer Single-Consumer queue.
 * Optimized for high-throughput DAW telemetry and command dispatch.
 */
template<typename T, uint32_t Size>
struct RingBuffer {
    static_assert((Size & (Size - 1)) == 0, "AURA | ERROR: RingBuffer size must be a power of two for bitwise optimization.");

    T buffer[Size];
    
    // INDUSTRIAL: Prevent 'False Sharing' by aligning atomic pointers to different cache lines (64 bytes).
    alignas(64) std::atomic<uint32_t> writePtr{0};
    alignas(64) std::atomic<uint32_t> readPtr{0};

    /**
     * @brief Pushes an item into the queue.
     * INDUSTRIAL: Using bitwise AND instead of modulo for O(1) index calculation.
     */
    bool push(const T& val) {
        uint32_t w = writePtr.load(std::memory_order_relaxed);
        uint32_t r = readPtr.load(std::memory_order_acquire);
        if (w - r >= Size) return false;
        
        buffer[w & (Size - 1)] = val;
        writePtr.store(w + 1, std::memory_order_release);
        return true;
    }

    /**
     * @brief Pops an item from the queue.
     */
    bool pop(T& val) {
        uint32_t r = readPtr.load(std::memory_order_relaxed);
        uint32_t w = writePtr.load(std::memory_order_acquire);
        if (r == w) return false;
        
        val = buffer[r & (Size - 1)];
        readPtr.store(r + 1, std::memory_order_release);
        return true;
    }

    bool isEmpty() const {
        return readPtr.load(std::memory_order_relaxed) == writePtr.load(std::memory_order_acquire);
    }

    // Reset is only valid at an ownership boundary where the producer and
    // consumer are stopped (project load/new-project). Keeping it explicit
    // prevents stale commands from a previous document being applied to the
    // first audio block of the next one.
    void clear() noexcept {
        const uint32_t write = writePtr.load(std::memory_order_acquire);
        readPtr.store(write, std::memory_order_release);
    }
};

} // namespace Aura::Core
