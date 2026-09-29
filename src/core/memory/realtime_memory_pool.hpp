#pragma once

#include <cstddef>
#include <cstdint>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Memory {

/** C++ lifetime and API adapter for the Rust-owned realtime arena allocator. */
class RealtimeMemoryPool {
public:
    static constexpr uint8_t kNumArenas = 8;
    static constexpr size_t kDefaultArenaSize = 1024 * 1024 * 4;

    static RealtimeMemoryPool& getInstance() {
        static RealtimeMemoryPool instance;
        return instance;
    }

    RealtimeMemoryPool(const RealtimeMemoryPool&) = delete;
    RealtimeMemoryPool& operator=(const RealtimeMemoryPool&) = delete;
    ~RealtimeMemoryPool() { hirari_realtime_memory_pool_destroy(m_state); }

    void reset(uint32_t frameIdx) noexcept {
        hirari_realtime_memory_pool_reset(m_state, frameIdx);
    }

    void* allocate(size_t sizeBytes) noexcept {
        return hirari_realtime_memory_pool_allocate(m_state, sizeBytes);
    }

private:
    RealtimeMemoryPool() : m_state(hirari_realtime_memory_pool_create()) {}
    void* m_state = nullptr;
};

} // namespace Hirari::Core::Memory
