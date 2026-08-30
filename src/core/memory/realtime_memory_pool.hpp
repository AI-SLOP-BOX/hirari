#pragma once
#include <vector>
#include <atomic>
#include <cstdint>
#include <array>
#include <cstring>

namespace Aura::Core::Memory {

/**
 * @class RealtimeMemoryPool
 * @brief Industrial Linear Allocator for RT-thread scratch memory.
 * HONEST FIX: Purged 'Harmonic Tension' scaling and implemented deterministic linear allocation.
 */
class RealtimeMemoryPool {
public:
    static constexpr uint8_t kNumArenas = 8;
    static constexpr size_t kDefaultArenaSize = 1024 * 1024 * 4; // 4MB per arena

    static RealtimeMemoryPool& getInstance() { static RealtimeMemoryPool i; return i; }

    /**
     * @brief Resets the current arena for the next processing frame.
     */
    void reset(uint32_t frameIdx) {
        uint32_t f = frameIdx % kNumArenas;
        m_heads[f].val.store(0, std::memory_order_release);
        m_frameIdx.store(f, std::memory_order_release);
    }

    /**
     * @brief RT-Safe linear allocation (lock-free).
     */
    void* allocate(size_t sizeBytes) {
        if (sizeBytes == 0 || sizeBytes > m_arenaSizes[0] || sizeBytes > SIZE_MAX - 63) return nullptr;
        uint32_t f = m_frameIdx.load(std::memory_order_acquire);

        // 64-byte alignment for SIMD safety
        size_t alignedSize = (sizeBytes + 63) & ~63;
        size_t head = m_heads[f].val.load(std::memory_order_relaxed);
        for (;;) {
            if (head > m_arenaSizes[f] - alignedSize) return nullptr;
            if (m_heads[f].val.compare_exchange_weak(head, head + alignedSize,
                                                     std::memory_order_relaxed,
                                                     std::memory_order_relaxed)) {
                return m_arenas[f] + head;
            }
        }
    }

private:
    RealtimeMemoryPool() {
        for (size_t i = 0; i < kNumArenas; ++i) {
            m_arenaSizes[i] = kDefaultArenaSize;
            m_arenas[i] = new uint8_t[kDefaultArenaSize];
            std::memset(m_arenas[i], 0, kDefaultArenaSize);
            m_heads[i].val.store(0);
        }
    }

    ~RealtimeMemoryPool() {
        for (size_t i = 0; i < kNumArenas; ++i) {
            delete[] m_arenas[i];
        }
    }

    struct alignas(64) ArenaHead {
        std::atomic<size_t> val{0};
    };

    std::array<uint8_t*, kNumArenas> m_arenas;
    std::array<size_t, kNumArenas> m_arenaSizes;
    std::array<ArenaHead, kNumArenas> m_heads;
    std::atomic<uint32_t> m_frameIdx{0};
};

} // namespace Aura::Core::Memory
