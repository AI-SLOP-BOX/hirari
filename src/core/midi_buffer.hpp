#pragma once

#include <vector>
#include <cstdint>
#include <cstddef>
#include <algorithm>
#include "plugins/midi_fragment_transport.hpp"
#include "rust_ffi.hpp"

namespace Hirari::Core {

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

static_assert(offsetof(MidiEvent, sampleOffset) == 0);
static_assert(offsetof(MidiEvent, size) == 8);
static_assert(offsetof(MidiEvent, data) == 12);
static_assert(offsetof(MidiEvent, articulationId) == 268);
static_assert(sizeof(MidiEvent) == 272);

/**
 * @class MidiBuffer
 * @brief High-performance collection of timestamped MIDI events.
 * HONEST FIX: Replaced raw byte vector with structured sample-accurate events.
 */
class MidiBuffer {
public:
    static constexpr size_t kMaxEventsPerBlock = 1024;

    MidiBuffer() : m_state(hirari_midi_buffer_create()) {}
    ~MidiBuffer() { hirari_midi_buffer_destroy(m_state); }
    MidiBuffer(const MidiBuffer&) = delete;
    MidiBuffer& operator=(const MidiBuffer&) = delete;

    /**
     * @brief RT-SAFE: No dynamic allocation.
     * HONEST FIX: Uses reserve() and size checks instead of raw push_back to prevent 
     * the allocator from triggering a 'Page Fault' spike in the audio thread.
     */
    void addEvent(uint64_t sampleOffset, const uint8_t* data, uint32_t size, uint8_t articulationId = 0) {
        hirari_midi_buffer_add(m_state, sampleOffset, data, size, articulationId);
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

    void addAllNotesOff(uint8_t channel, uint64_t sampleOffset) {
        if (channel == 0 || channel > 16) return;
        uint8_t data[3] = { static_cast<uint8_t>(0xB0 | (channel - 1)), 123, 0 };
        addEvent(sampleOffset, data, 3, 0);
    }

    /**
     * @brief Stable, in-place timestamp sort with safe same-sample note order.
     *
     * Insertion sort is intentional here: the block is already nearly sorted
     * in normal playback, it performs no allocation on the audio thread, and
     * preserves producer order among events with the same priority. At an
     * identical sample, All Notes Off precedes Note Off, other MIDI events,
     * and Note On. This prevents a boundary Note Off from killing a newly
     * retriggered note while retaining stable order for controllers and other
     * events within their priority class.
     */
    void sort() {
        hirari_midi_buffer_sort_owned(m_state);
    }

    void clear() {
        hirari_midi_buffer_clear(m_state);
    }

    bool overflowed() const noexcept {
        return hirari_midi_buffer_overflowed(m_state);
    }

    uint64_t droppedEvents() const noexcept {
        // A non-destructive telemetry snapshot is returned by the Rust owner.
        return hirari_midi_buffer_dropped_count(m_state);
    }

    uint64_t takeDroppedEvents() noexcept {
        return hirari_midi_buffer_take_dropped(m_state);
    }

    uint64_t takeOversizeEvents() noexcept {
        return hirari_midi_buffer_take_oversize(m_state);
    }

    // Counts rejected SysEx/MIDI 2.0-shaped payloads separately from ordinary
    // malformed oversized events. This is telemetry, not silent truncation.
    uint64_t takeExtendedEvents() noexcept {
        return hirari_midi_buffer_take_extended(m_state);
    }

    size_t remainingCapacity() const noexcept {
        return hirari_midi_buffer_remaining_capacity(m_state);
    }

    bool tryAddEvent(const MidiEvent& event) {
        return hirari_midi_buffer_copy(m_state, &event);
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
            hirari_midi_buffer_reject_extended(m_state);
            return Plugins::MidiFragmentReassembler::Result::Oversize;
        }
        addEvent(reassembler.sampleOffset(), reassembler.data(),
                 static_cast<uint32_t>(reassembler.size()), reassembler.articulationId());
        return result;
    }

    bool takeOverflowed() noexcept {
        return hirari_midi_buffer_take_overflowed(m_state);
    }
    
    const MidiEvent* getEvents() const {
        return static_cast<const MidiEvent*>(hirari_midi_buffer_event_data(m_state));
    }
    MidiEvent* getMutableEvents() {
        return static_cast<MidiEvent*>(hirari_midi_buffer_event_data(m_state));
    }
    size_t size() const { return hirari_midi_buffer_event_count(m_state); }
    void* rustStateHandle() noexcept { return m_state; }
    const void* rustStateHandle() const noexcept { return m_state; }
    const MidiEvent* begin() const { return getEvents(); }
    const MidiEvent* end() const { return getEvents() + size(); }

    class Iterator {
    public:
        explicit Iterator(const MidiBuffer& buffer) : m_buffer(buffer) {}
        bool getNextEvent(uint32_t& offset, uint8_t* data, uint32_t& size) {
            if (m_index >= m_buffer.size()) return false;
            const auto& event = m_buffer.getEvents()[m_index++];
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
    void* m_state = nullptr;
};

} // namespace Hirari::Core
