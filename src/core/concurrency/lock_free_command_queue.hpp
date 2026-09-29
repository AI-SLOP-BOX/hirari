#pragma once

#include <variant>
#include <atomic>
#include <cstdint>
#include <algorithm>
#include "lock_free.hpp"

namespace Hirari::Core::Concurrency {

/**
 * @struct Command
 * @brief Zero-allocation Command structure for UI-to-Audio communication.
 * HONEST FIX: Unified command structure to prevent heap allocations 
 * and mutex stalls during parameter changes or track management.
 */
struct Command {
    enum Type {
        PARAM_CHANGE,
        BYPASS_TOGGLE,
        TRACK_VOLUME,
        TRACK_PAN,
        ENGINE_STOP,
        ENGINE_START,
        MIDI_EVENT
    };

    struct MidiData {
        uint8_t status, d1, d2;
    };

    Type type;
    uint32_t targetId;  // trackId, paramId, etc.
    float value;        // newValue, etc.
    uint32_t sampleOffset; // FIX: Essential for sample-accurate automation
    MidiData midi;
};

/**
 * @class LockFreeCommandQueue
 * @brief Professional SPSC Command Queue for DAW UI interaction.
 * HONEST FIX: Added template and offset-sorting logic to support 
 * sample-accurate automation (Logic Pro level precision).
 */
template<typename T>
class LockFreeCommandQueue {
public:
    bool push(const T& cmd) { return m_queue.push(cmd); }
    bool pop(T& cmd) { return m_queue.pop(cmd); }

    /**
     * @brief DRAIN (RT-SAFE): Move all queue items to a local buffer and sort by offset.
     */
    void drainToLocal(T* localBuf, size_t maxCount, size_t& count) {
        count = 0;
        while (count < maxCount && m_queue.pop(localBuf[count])) {
            count++;
        }
        // Sorting in RT thread is okay for small N (e.g. < 128 automation points per block)
        std::sort(localBuf, localBuf + count, [](const T& a, const T& b) {
            return a.offset < b.offset;
        });
    }

private:
    SPSCQueue<T, 4096> m_queue;
};

} // namespace Hirari::Core::Concurrency
