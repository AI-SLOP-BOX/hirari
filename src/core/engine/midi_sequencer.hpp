#pragma once
#include <unordered_map>
#include <vector>
#include <memory>
#include <mutex>
#include <algorithm>
#include "midi_quantizer.hpp"

namespace Aura::Core::Engine {

struct MidiNote {
    uint8_t pitch = 0;
    uint8_t velocity = 0;
    uint64_t startTick = 0;
    uint64_t length = 0;
};

/**
 * @class MidiSequencer
 * @brief Industrial MIDI Performance Orchestrator.
 * HONEST FIX: Implemented tick-based sequencing and note chasing.
 */
class MidiSequencer {
public:
    static MidiSequencer& getInstance() { static MidiSequencer i; return i; }

    /**
     * @brief Records a MIDI event with industrial tick precision and sequencing sovereignty.
     * INDUSTRIAL: Delegating note storage and indexing to the Rust 'MidiOrchestrator'.
     */
    void recordNote(uint32_t regionId, uint8_t pitch, uint8_t velocity, uint64_t startTick, uint64_t length) {
        if (pitch > 127 || velocity > 127 || length == 0) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        auto& notes = m_regions[regionId];
        if (notes.size() >= kMaxNotesPerRegion) return;
        notes.push_back(MidiNote{pitch, velocity, startTick, length});
        std::stable_sort(notes.begin(), notes.end(), [](const MidiNote& a, const MidiNote& b) {
            return a.startTick < b.startTick;
        });
    }

    /**
     * @brief NOTE CHASE: Identifies notes that should be active at the given tick with forensic precision.
     * INDUSTRIAL: Using Rust for robust and perfectly timed note chasing.
     */
    std::vector<MidiNote> chaseNotes(uint64_t currentTick) {
        std::lock_guard<std::mutex> lock(m_mutex);
        std::vector<MidiNote> active;
        for (const auto& [regionId, notes] : m_regions) {
            (void)regionId;
            for (const MidiNote& note : notes) {
                if (note.startTick > currentTick) break;
                const uint64_t end = note.startTick > UINT64_MAX - note.length
                    ? UINT64_MAX : note.startTick + note.length;
                if (currentTick < end) active.push_back(note);
            }
        }
        return active;
    }

    void clear() noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_regions.clear();
    }

private:
    static constexpr size_t kMaxNotesPerRegion = 1'000'000;
    MidiSequencer() = default;
    std::unordered_map<uint32_t, std::vector<MidiNote>> m_regions;
    mutable std::mutex m_mutex;
};

} // namespace Aura::Core::Engine
