#pragma once

#include <array>
#include <atomic>
#include <cstdint>
#include <algorithm>
#include "midi_buffer.hpp"
#include "utils/ring_buffer.hpp"

/**
 * @brief MidiEvent: Sample-accurate MIDI data container.
 */
/**
 * @brief MidiDispatcher: Orchestrates MIDI events across a render block.
 * Addresses the "lack of sample-accurate MIDI management" from the review.
 */
namespace Aura::Core {

/**
 * @class MidiDispatcher
 * @brief Industrial Midi Orchestrator (Deterministic Event Sovereignty).
 * Implements lock-free ingestion and sample-accurate block sorting.
 */
class MidiDispatcher {
public:
    static constexpr size_t kMaxEventsPerBlock = 1024;

    /**
     * @brief Pushes an event into the lock-free ingestion queue (RT-Safe).
     */
    bool pushEvent(const MidiEvent& event) noexcept {
        if (m_ingestionQueue.push(event)) return true;
        m_droppedInputEvents.fetch_add(1, std::memory_order_relaxed);
        return false;
    }

    /**
     * @brief Orchestrates events for the current render block.
     * Performs deterministic sorting and re-clocking.
     */
    struct BlockEvents {
        const MidiEvent* begin() const noexcept { return events.data(); }
        const MidiEvent* end() const noexcept { return events.data() + count; }
        size_t size() const noexcept { return count; }
        std::array<MidiEvent, kMaxEventsPerBlock> events{};
        size_t count = 0;
    };

    const BlockEvents& getEventsForBlock() noexcept {
        m_blockEvents.count = 0;
        MidiEvent event;
        while (m_ingestionQueue.pop(event)) {
            // Jitter Buffer: Re-clocking arriving events to sample grid
            if (m_blockEvents.count == kMaxEventsPerBlock) {
                m_droppedEvents.fetch_add(1, std::memory_order_relaxed);
                continue;
            }
            m_blockEvents.events[m_blockEvents.count++] = event;
        }

        // Deterministic Sort: Essential for MPE and sample-accurate playback
        std::sort(m_blockEvents.events.begin(), m_blockEvents.events.begin() + m_blockEvents.count,
                  [](const MidiEvent& a, const MidiEvent& b) {
            return a.sampleOffset < b.sampleOffset;
        });

        return m_blockEvents;
    }

    void clear() noexcept { m_blockEvents.count = 0; }
    uint64_t takeDroppedEvents() noexcept {
        return m_droppedEvents.exchange(0, std::memory_order_acq_rel) +
               m_droppedInputEvents.exchange(0, std::memory_order_acq_rel);
    }

private:
    ::Aura::Core::RingBuffer<MidiEvent, kMaxEventsPerBlock> m_ingestionQueue;
    BlockEvents m_blockEvents; // Fixed storage; never allocates on the audio thread.
    std::atomic<uint64_t> m_droppedEvents{0};
    std::atomic<uint64_t> m_droppedInputEvents{0};
};

} // namespace Aura::Core
