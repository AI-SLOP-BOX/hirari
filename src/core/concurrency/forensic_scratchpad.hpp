#pragma once
#include <cstdint>
#include <cstddef>
#include <new>

namespace Aura::Core::Concurrency {

/**
 * @class ForensicScratchpad
 * @brief Industrial-grade, thread-local memory arena for real-time DSP.
 * INDUSTRIAL: Eliminates heap allocation in the audio thread by providing a pre-allocated 1MB workspace.
 * Uses a simple pointer-increment allocation strategy for O(1) performance and zero-fragmentation.
 */
class ForensicScratchpad {
public:
    /**
     * @brief Access the thread-local instance of the scratchpad.
     */
    static ForensicScratchpad& getThreadLocal() {
        static thread_local ForensicScratchpad instance;
        return instance;
    }

    /**
     * @brief Allocates a block of memory from the arena.
     * INDUSTRIAL: Aligned to 64 bytes for cache-line performance.
     */
    void* allocate(size_t size) {
        // Force 64-byte alignment for SIMD and Cache-line friendliness.
        const size_t alignedSize = (size + 63) & ~size_t(63);
        
        if (m_offset + alignedSize > Capacity) {
            // INDUSTRIAL: In a production crash, we would log this 'Sovereign Overflow'.
            return nullptr; 
        }

        void* ptr = m_buffer + m_offset;
        m_offset += alignedSize;
        return ptr;
    }

    /**
     * @brief Type-safe array allocation.
     */
    template<typename T>
    T* allocateArray(size_t count) {
        return static_cast<T*>(allocate(sizeof(T) * count));
    }

    /**
     * @brief Resets the allocation pointer to the start of the arena.
     * INDUSTRIAL: Called at the end of each processBlock to recycle memory without deletion.
     */
    void reset() {
        m_offset = 0;
    }

private:
    static constexpr size_t Capacity = 1024 * 1024; // 1MB per thread

    ForensicScratchpad() : m_offset(0) {
        // INDUSTRIAL: Pre-allocate the entire arena on thread initialization.
        m_buffer = new (std::align_val_t(64)) uint8_t[Capacity];
    }

    ~ForensicScratchpad() {
        // This won't actually be called on thread_local in many scenarios,
        // but we include it for structural integrity.
        operator delete[](m_buffer, std::align_val_t(64));
    }

    uint8_t* m_buffer;
    size_t m_offset;

    // Prevent copying
    ForensicScratchpad(const ForensicScratchpad&) = delete;
    ForensicScratchpad& operator=(const ForensicScratchpad&) = delete;
};

} // namespace Aura::Core::Concurrency
