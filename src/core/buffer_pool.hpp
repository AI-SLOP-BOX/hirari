#pragma once
#include <array>
#include <atomic>
#include <vector>
#include "concurrency/lock_free.hpp"

namespace Aura::Core {

/**
 * @class BufferPool
 * @brief Truly lock-free, zero-allocation, flat-memory buffer recycler.
 * Resolves std::vector pointer invalidation and heap allocation risks in real-time threads
 * by pre-allocating a single contiguous flat-memory buffer slice array on startup.
 */
class BufferPool {
public:
    static constexpr size_t kMaxBuffers = 64;
    static constexpr size_t kSamplesPerBuffer = 8192;

    BufferPool() {
        m_flatBuffers.fill(0.0f);
        for (size_t i = 0; i < kMaxBuffers; ++i) {
            m_availableIndices.push(static_cast<uint32_t>(i));
        }
    }

    /**
     * @struct PooledBuffer
     * @brief RAII handle for a buffer slice returning to the pool automatically on destruction.
     */
    struct PooledBuffer {
        float* data = nullptr;
        uint32_t index = 0xFFFFFFFF;
        BufferPool* owner = nullptr;

        ~PooledBuffer() {
            if (owner && index != 0xFFFFFFFF) {
                owner->m_availableIndices.push(index);
            }
        }
        
        // Disable copy
        PooledBuffer(const PooledBuffer&) = delete;
        PooledBuffer& operator=(const PooledBuffer&) = delete;
        
        // Enable move
        PooledBuffer(PooledBuffer&& other) noexcept 
            : data(other.data), index(other.index), owner(other.owner) {
            other.owner = nullptr;
            other.index = 0xFFFFFFFF;
        }

        PooledBuffer& operator=(PooledBuffer&& other) noexcept {
            if (this != &other) {
                if (owner && index != 0xFFFFFFFF) {
                    owner->m_availableIndices.push(index);
                }
                data = other.data;
                index = other.index;
                owner = other.owner;
                other.owner = nullptr;
                other.index = 0xFFFFFFFF;
            }
            return *this;
        }
    };

    /**
     * @brief Acquires a buffer slice from the pool without any locks or allocations.
     */
    PooledBuffer acquire() {
        auto idx = m_availableIndices.pop();
        if (idx) {
            float* slicePtr = &m_flatBuffers[*idx * kSamplesPerBuffer];
            return { slicePtr, *idx, this };
        }
        return { nullptr, 0xFFFFFFFF, nullptr };
    }

private:
    // Single contiguous flat array mapping to dynamic virtual memory (100% cache-friendly)
    std::array<float, kMaxBuffers * kSamplesPerBuffer> m_flatBuffers;
    Concurrency::SPSCQueue<uint32_t, kMaxBuffers> m_availableIndices;
};

} // namespace Aura::Core
