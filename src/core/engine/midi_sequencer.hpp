#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>
#include <vector>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

struct MidiNote {
    uint8_t pitch = 0;
    uint8_t velocity = 0;
    uint64_t startTick = 0;
    uint64_t length = 0;
};

static_assert(sizeof(MidiNote) == 24, "MidiNote must match the Rust sequencer ABI");
static_assert(offsetof(MidiNote, startTick) == 8 && offsetof(MidiNote, length) == 16,
              "MidiNote field offsets must match the Rust sequencer ABI");

/**
 * @brief C++ compatibility facade for the Rust MIDI note sequencer.
 * Region storage, ordering, locking, and note chasing live in Rust.
 */
class MidiSequencer {
public:
    static MidiSequencer& getInstance() { static MidiSequencer instance; return instance; }

    void recordNote(uint32_t regionId, uint8_t pitch, uint8_t velocity,
                    uint64_t startTick, uint64_t length) {
        (void)hirari_midi_sequencer_record(
            m_state, regionId, pitch, velocity, startTick, length);
    }

    std::vector<MidiNote> chaseNotes(uint64_t currentTick) const {
        void* snapshot = hirari_midi_sequencer_chase_snapshot(m_state, currentTick);
        if (!snapshot) return {};
        std::unique_ptr<void, decltype(&hirari_midi_sequencer_snapshot_destroy)>
            snapshotGuard(snapshot, &hirari_midi_sequencer_snapshot_destroy);
        const size_t count = hirari_midi_sequencer_snapshot_count(snapshot);
        std::vector<MidiNote> active(count);
        if (!hirari_midi_sequencer_snapshot_copy(snapshot, active.data(), active.size())) {
            return {};
        }
        return active;
    }

    void clear() noexcept { hirari_midi_sequencer_clear(m_state); }

private:
    MidiSequencer() : m_state(hirari_midi_sequencer_create()) {}
    ~MidiSequencer() { hirari_midi_sequencer_destroy(m_state); }
    MidiSequencer(const MidiSequencer&) = delete;
    MidiSequencer& operator=(const MidiSequencer&) = delete;

    void* m_state = nullptr;
};

} // namespace Hirari::Core::Engine
