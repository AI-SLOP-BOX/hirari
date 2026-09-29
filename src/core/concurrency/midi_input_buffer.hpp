#pragma once

#include <array>
#include <cstddef>
#include <cstdint>

#include "../rust_ffi.hpp"

namespace Hirari::Core::Concurrency {

/** Host-facing API for the Rust-owned lock-free hardware MIDI queue. */
class MidiInputBuffer {
public:
    static MidiInputBuffer& getInstance() {
        static MidiInputBuffer instance;
        return instance;
    }

    struct RawMidiEvent {
        uint8_t status;
        uint8_t data1;
        uint8_t data2;
        uint64_t timestamp;
    };
    static_assert(offsetof(RawMidiEvent, timestamp) == 8);
    static_assert(sizeof(RawMidiEvent) == 16);

    MidiInputBuffer(const MidiInputBuffer&) = delete;
    MidiInputBuffer& operator=(const MidiInputBuffer&) = delete;

    void pushEvent(uint8_t status, uint8_t data1, uint8_t data2, uint64_t timestamp) {
        hirari_midi_input_buffer_push(m_state, status, data1, data2, timestamp);
    }

    bool addBlacklistRule(uint8_t statusMask, uint8_t statusValue,
                          uint8_t data1 = 0xFF, uint8_t data2 = 0xFF) {
        return hirari_midi_input_buffer_add_blacklist_rule(
            m_state, statusMask, statusValue, data1, data2);
    }

    void clearBlacklist() { hirari_midi_input_buffer_clear_blacklist(m_state); }
    uint64_t droppedEventCount() const {
        return hirari_midi_input_buffer_dropped(m_state);
    }

    size_t pullEvents(std::array<RawMidiEvent, 512>& outputBuffer) {
        return hirari_midi_input_buffer_pull(
            m_state, outputBuffer.data(), outputBuffer.size());
    }

private:
    MidiInputBuffer() : m_state(hirari_midi_input_buffer_create()) {}
    ~MidiInputBuffer() { hirari_midi_input_buffer_destroy(m_state); }

    void* m_state = nullptr;
};

} // namespace Hirari::Core::Concurrency
