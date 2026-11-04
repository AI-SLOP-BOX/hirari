#pragma once
#include <vector>
#include <array>
#include <string>
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <memory>
#include <utility>
#include <mutex>
#include <new>
#include "../rust_ffi.hpp"
#include "../project_automation.hpp"

namespace Hirari::Core::Engine {


// Shared invalidation for cheap project-layout polling. The realtime thread
// only stores a dirty bit when it queues write-automation points; the
// control-side reader folds that bit into the monotonic revision.
struct ProjectLayoutRevision {
    std::atomic<uint64_t> value{1};
    std::atomic<bool> realtimeAutomationDirty{false};

    void markChanged() noexcept {
        value.fetch_add(1, std::memory_order_release);
    }

    uint64_t current() noexcept {
        if (realtimeAutomationDirty.exchange(false, std::memory_order_acq_rel)) {
            markChanged();
        }
        return value.load(std::memory_order_acquire);
    }
};

// The Rust audio storage owns the samples, the channel view and the dimensions;
// C++ keeps only a lifetime handle to it, so a published freeze snapshot cannot
// outlive its buffer and no sample policy is duplicated here.
using SharedAudioStorage = std::shared_ptr<void>;

inline SharedAudioStorage shareAudioStorage(void* storage) {
    return SharedAudioStorage(storage,
        [](void* handle) { hirari_audio_buffer_storage_destroy(handle); });
}

// The reserve status is the storage's own allocation policy: 1 and 5 mean the
// realtime guard declined the growth, 2 an impossible size and any other
// non-zero an exhausted allocation.
inline bool prepareAudioStorage(void* storage, uint32_t channels, uint32_t samples) {
    const uint8_t status =
        hirari_audio_buffer_storage_reserve_buffer(storage, channels, samples);
    if (status == 1 || status == 5) return false;
    if (status == 2) throw std::bad_array_new_length();
    if (status != 0) throw std::bad_alloc();
    return hirari_audio_buffer_storage_resize(storage, channels, samples);
}

inline SharedAudioStorage makeSharedAudioStorage(uint32_t channels, uint32_t samples) {
    auto storage = shareAudioStorage(hirari_audio_buffer_storage_create());
    return prepareAudioStorage(storage.get(), channels, samples) ? storage
                                                                 : SharedAudioStorage{};
}

/**
 * @class Track
 * @brief High-performance track orchestration engine.
 * HONEST FIX: Optimized region lookup using sorted-list binary search.
 */class Track {
    #include "track_part_1a.inc"
    #include "track_part_1b.inc"
    #include "track_part_2.inc"
    #include "track_part_3a.inc"
    #include "track_part_3b.inc"
    #include "track_part_4.inc"

} // namespace Hirari::Core::Engine
