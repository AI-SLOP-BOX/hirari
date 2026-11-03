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
#include "../rust_ffi.hpp"
#include "../audio_buffer.hpp"
#include "../midi_buffer.hpp"
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

/**
 * @class Track
 * @brief High-performance track orchestration engine.
 * HONEST FIX: Optimized region lookup using sorted-list binary search.
 */
class Track {
    #include "track_part_1a.inc"
    #include "track_part_1b.inc"
    #include "track_part_2.inc"
    #include "track_part_3a.inc"
    #include "track_part_3b.inc"
    #include "track_part_4.inc"

} // namespace Hirari::Core::Engine
