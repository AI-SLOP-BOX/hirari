#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>
#include <vector>
#include "midi_quantizer.hpp"
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

static_assert(offsetof(MIDINote, startTick) == 0 && offsetof(MIDINote, lengthTicks) == 8,
              "MIDINote fields must match the Rust capture ABI");
static_assert(offsetof(MIDINote, note) == 16 && offsetof(MIDINote, velocity) == 17,
              "MIDINote fields must match the Rust capture ABI");

/** C++ compatibility facade for Rust-backed retrospective MIDI capture. */
class RetrospectiveMidiCapture {
public:
    static RetrospectiveMidiCapture& getInstance() {
        static RetrospectiveMidiCapture instance;
        return instance;
    }

    void bufferEvent(uint32_t trackId, uint8_t status, uint8_t data1,
                     uint8_t data2, uint64_t tick) {
        hirari_retrospective_midi_record(
            m_state, trackId, status, data1, data2, tick);
    }

    std::vector<MIDINote> flush(uint64_t currentTick, uint64_t lookbackTicks) const {
        void* snapshot = hirari_retrospective_midi_flush_snapshot(
            m_state, currentTick, lookbackTicks);
        if (!snapshot) return {};
        std::unique_ptr<void, decltype(&hirari_retrospective_midi_snapshot_destroy)>
            snapshotGuard(snapshot, &hirari_retrospective_midi_snapshot_destroy);
        const size_t count = hirari_retrospective_midi_snapshot_count(snapshot);
        std::vector<MIDINote> notes(count);
        if (!hirari_retrospective_midi_snapshot_copy(snapshot, notes.data(), notes.size())) {
            return {};
        }
        return notes;
    }

private:
    RetrospectiveMidiCapture() : m_state(hirari_retrospective_midi_create()) {}
    ~RetrospectiveMidiCapture() { hirari_retrospective_midi_destroy(m_state); }
    RetrospectiveMidiCapture(const RetrospectiveMidiCapture&) = delete;
    RetrospectiveMidiCapture& operator=(const RetrospectiveMidiCapture&) = delete;

    void* m_state = nullptr;
};

} // namespace Hirari::Core::Engine
