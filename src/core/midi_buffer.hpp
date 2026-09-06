#pragma once

#include <vector>
#include <cstdint>
#include <algorithm>
#include <atomic>
#include "plugins/midi_fragment_transport.hpp"

namespace Aura::Core {

/**
 * @struct MidiEvent
 * @brief Professional Sample-Accurate MIDI event with 64-bit timestamp.
 * Essential for VST/AU instrument timing.
 */
struct MidiEvent {
    uint64_t sampleOffset; // Relative to the start of the current audio block
    uint32_t size;         // Size of data in bytes (usually 3 for Note On/Off)
    // 256 bytes covers MIDI 2.0 UMP packets and bounded SysEx chunks. Larger
    // SysEx payloads are rejected and reported instead of being truncated.
    uint8_t data[256];
    uint8_t articulationId = 0; // 0 = Default, 1+ = Technique ID
};

/**
 * @class MidiBuffer
 * @brief High-performance collection of timestamped MIDI events.
 * HONEST FIX: Replaced raw byte vector with structured sample-accurate events.
 */
class MidiBuffer {
public:
    static constexpr size_t kMaxEventsPerBlock = 1024;

    MidiBuffer() : m_count(0) {}

    /**
     * @brief RT-SAFE: No dynamic allocation.
     * HONEST FIX: Uses reserve() and size checks instead of raw push_back to prevent 
     * the allocator from triggering a 'Page Fault' spike in the audio thread.
     */
    void addEvent(uint64_t sampleOffset, const uint8_t* data, uint32_t size, uint8_t articulationId = 0) {
        if (size > sizeof(MidiEvent::data)) {
            m_oversizeEvents.fetch_add(1, std::memory_order_relaxed);
            if (data && size > 0 && (data[0] == 0xf0 || size >= 4))
                m_extendedEvents.fetch_add(1, std::memory_order_relaxed);
            m_overflowed.store(true, std::memory_order_release);
            return;
        }
        if (size != 0 && data == nullptr) return;
        if (m_count >= kMaxEventsPerBlock) {
            // Dropping is still the only RT-safe fallback once the preallocated
            // block is full, but it must be observable by the host/UI.
            m_droppedEvents.fetch_add(1, std::memory_order_relaxed);
            m_overflowed.store(true, std::memory_order_release);
            return;
        }
        
        MidiEvent& ev = m_events[m_count++];
        ev.sampleOffset = sampleOffset;
        ev.size = size;
        ev.articulationId = articulationId;
        if (size > 0) std::copy(data, data + size, ev.data);
        if (size < sizeof(ev.data))
            std::fill(ev.data + size, ev.data + sizeof(ev.data), uint8_t{0});
    }

    void addNoteOn(uint8_t channel, uint8_t pitch, uint8_t velocity, uint64_t sampleOffset, uint8_t articulationId = 0) {
        if (channel == 0 || channel > 16) return;
        uint8_t data[3] = { static_cast<uint8_t>(0x90 | (channel - 1)), pitch, velocity };
        addEvent(sampleOffset, data, 3, articulationId);
    }

    void addNoteOff(uint8_t channel, uint8_t pitch, uint64_t sampleOffset) {
        if (channel == 0 || channel > 16) return;
        uint8_t data[3] = { static_cast<uint8_t>(0x80 | (channel - 1)), pitch, 0 };
        addEvent(sampleOffset, data, 3, 0);
    }

    /**
     * @brief Stable, in-place timestamp sort.
     *
     * Insertion sort is intentional here: the block is already nearly sorted
     * in normal playback, it performs no allocation on the audio thread, and
     * (unlike an unstable Shell sort) preserves producer order for events at
     * the same sample. That ordering is observable for note-off/note-on and
     * articulation changes sharing a boundary.
     */
    void sort() {
        if (m_count < 2) return;
        for (size_t i = 1; i < m_count; ++i) {
            MidiEvent current = m_events[i];
            size_t j = i;
            while (j > 0 && m_events[j - 1].sampleOffset > current.sampleOffset) {
                m_events[j] = m_events[j - 1];
                --j;
            }
            m_events[j] = current;
        }
    }

    void clear() {
        m_count = 0;
        m_droppedEvents.store(0, std::memory_order_relaxed);
        m_oversizeEvents.store(0, std::memory_order_relaxed);
        m_extendedEvents.store(0, std::memory_order_relaxed);
        m_overflowed.store(false, std::memory_order_release);
    }

    bool overflowed() const noexcept {
        return m_overflowed.load(std::memory_order_acquire);
    }

    uint64_t droppedEvents() const noexcept {
        return m_droppedEvents.load(std::memory_order_relaxed);
    }

    uint64_t takeDroppedEvents() noexcept {
        return m_droppedEvents.exchange(0, std::memory_order_acq_rel);
    }

    uint64_t takeOversizeEvents() noexcept {
        return m_oversizeEvents.exchange(0, std::memory_order_acq_rel);
    }

    // Counts rejected SysEx/MIDI 2.0-shaped payloads separately from ordinary
    // malformed oversized events. This is telemetry, not silent truncation.
    uint64_t takeExtendedEvents() noexcept {
        return m_extendedEvents.exchange(0, std::memory_order_acq_rel);
    }

    size_t remainingCapacity() const noexcept {
        return m_count < kMaxEventsPerBlock ? kMaxEventsPerBlock - m_count : 0;
    }

    bool tryAddEvent(const MidiEvent& event) {
        if (event.size > sizeof(event.data)) {
            m_oversizeEvents.fetch_add(1, std::memory_order_relaxed);
            if (event.size >= 4 || (event.size > 0 && event.data[0] == 0xf0))
                m_extendedEvents.fetch_add(1, std::memory_order_relaxed);
            m_overflowed.store(true, std::memory_order_release);
            return false;
        }
        if (m_count >= kMaxEventsPerBlock) {
            m_droppedEvents.fetch_add(1, std::memory_order_relaxed);
            m_overflowed.store(true, std::memory_order_release);
            return false;
        }
        MidiEvent& stored = m_events[m_count++];
        stored = event;
        if (stored.size < sizeof(stored.data))
            std::fill(stored.data + stored.size,
                      stored.data + sizeof(stored.data), uint8_t{0});
        return true;
    }

    /// Completes a fragmented SysEx/UMP message into the realtime mailbox
    /// only when it fits the bounded event slot. Larger messages are rejected
    /// and counted instead of being silently truncated.
    Plugins::MidiFragmentReassembler::Result addFragment(
        Plugins::MidiFragmentReassembler& reassembler,
        const Plugins::MidiFragmentReassembler::Fragment& fragment,
        Plugins::MidiExtendedMessageRing* extended = nullptr) {
        const auto result = reassembler.push(fragment);
        if (result != Plugins::MidiFragmentReassembler::Result::Complete) return result;
        if (reassembler.size() > sizeof(MidiEvent::data)) {
            if (extended != nullptr && extended->push(
                    reassembler.sampleOffset(), reassembler.articulationId(),
                    reassembler.data(), reassembler.size())) {
                return result;
            }
            m_oversizeEvents.fetch_add(1, std::memory_order_relaxed);
            m_extendedEvents.fetch_add(1, std::memory_order_relaxed);
            m_overflowed.store(true, std::memory_order_release);
            return Plugins::MidiFragmentReassembler::Result::Oversize;
        }
        addEvent(reassembler.sampleOffset(), reassembler.data(),
                 static_cast<uint32_t>(reassembler.size()), reassembler.articulationId());
        return result;
    }

    bool takeOverflowed() noexcept {
        return m_overflowed.exchange(false, std::memory_order_acq_rel);
    }
    
    const MidiEvent* getEvents() const { return m_events; }
    MidiEvent* getMutableEvents() { return m_events; }
    size_t size() const { return m_count; }
    const MidiEvent* begin() const { return m_events; }
    const MidiEvent* end() const { return m_events + m_count; }

    class Iterator {
    public:
        explicit Iterator(const MidiBuffer& buffer) : m_buffer(buffer) {}
        bool getNextEvent(uint32_t& offset, uint8_t* data, uint32_t& size) {
            if (m_index >= m_buffer.m_count) return false;
            const auto& event = m_buffer.m_events[m_index++];
            offset = event.sampleOffset > UINT32_MAX
                ? UINT32_MAX : static_cast<uint32_t>(event.sampleOffset);
            size = std::min<uint32_t>(event.size, static_cast<uint32_t>(sizeof(event.data)));
            if (size > 0 && data != nullptr) {
                std::copy(event.data, event.data + size, data);
            }
            return true;
        }
    private:
        const MidiBuffer& m_buffer;
        size_t m_index = 0;
    };

private:
    MidiEvent m_events[kMaxEventsPerBlock];
    size_t m_count;
    std::atomic<uint64_t> m_droppedEvents{0};
    std::atomic<uint64_t> m_oversizeEvents{0};
    std::atomic<uint64_t> m_extendedEvents{0};
    std::atomic<bool> m_overflowed{false};
};

} // namespace Aura::Core
