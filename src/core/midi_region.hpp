#pragma once
#include <vector>
#include <cstdint>
#include <cstddef>
#include <string>
#include <memory>
#include <algorithm>
#include <cmath>
#include "audio_types.hpp"
#include "midi_buffer.hpp"

namespace Hirari::Core {

static_assert(sizeof(MIDINote) == 24 && offsetof(MIDINote, pitch) == 0 &&
              offsetof(MIDINote, velocity) == 1 && offsetof(MIDINote, startBeat) == 8 &&
              offsetof(MIDINote, lengthBeats) == 16,
              "MIDINote must match the Rust MIDI region ABI");

/**
 * @class MidiRegion
 * @brief Industrial MIDI Orchestrator for Hirari Studio Pro.
 */
class MidiRegion {
public:
    MidiRegion(uint32_t id, const std::string& name, double startBeat = 0, double lengthBeats = 4.0) 
        : m_id(id), m_name(name), m_startBeat(startBeat), m_lengthBeats(lengthBeats),
          m_noteState(hirari_midi_region_state_create(nullptr, 0)) {}

    MidiRegion(const std::vector<MIDINote>& notes, double startBeat, double lengthBeats)
        : m_id(0), m_name("Generated"), m_startBeat(startBeat), m_lengthBeats(lengthBeats),
          m_noteState(hirari_midi_region_state_create(notes.data(), notes.size())) {}

    ~MidiRegion() { hirari_midi_region_state_destroy(m_noteState); }

    uint32_t getId() const { return m_id; }
    const std::string& getName() const { return m_name; }

    void addNote(MIDINote n) {
        (void)hirari_midi_region_state_add(m_noteState, &n);
    }
    bool removeNote(uint32_t index) {
        return hirari_midi_region_state_remove(m_noteState, index);
    }
    void removeNotesAt(double beat, int pitch, double tolerance = 0.125) {
        hirari_midi_region_state_remove_notes_at(m_noteState, beat, pitch, tolerance);
    }

    void setMutedAt(double beat, int pitch, bool muted) {
        hirari_midi_region_state_set_muted_at(m_noteState, beat, pitch, muted);
    }
    bool copyProcessedNotes(std::vector<MIDINote>& destination) const {
        if (!m_noteState) return false;
        for (unsigned attempt = 0; attempt < 4; ++attempt) {
            const size_t capacity = hirari_midi_region_state_count(m_noteState);
            destination.resize(capacity);
            const size_t copied = hirari_midi_region_state_copy(
                m_noteState, destination.data(), capacity);
            if (copied <= capacity) {
                destination.resize(copied);
                return true;
            }
        }
        destination.clear();
        return false;
    }

    bool updateNote(size_t index, const MIDINote& note) {
        return hirari_midi_region_state_update(m_noteState, index, &note);
    }

    bool replaceNotes(const std::vector<MIDINote>& notes) {
        return hirari_midi_region_state_replace(m_noteState, notes.data(), notes.size());
    }

    void transpose(int semitones, double startBeat = -1.0, double endBeat = -1.0) {
        hirari_midi_region_state_transpose(m_noteState, semitones, startBeat, endBeat);
    }

    void quantize(double grid, double strength = 1.0, double startBeat = -1.0, double endBeat = -1.0) {
        (void)hirari_midi_region_state_quantize(
            m_noteState, grid, strength, startBeat, endBeat);
    }
    
    double getStartBeat() const { return m_startBeat; }
    double getLengthBeats() const { return m_lengthBeats; }

private:
    uint32_t m_id;
    std::string m_name;
    double m_startBeat;
    double m_lengthBeats;
    void* m_noteState = nullptr;
};

using MIDIRegion = MidiRegion;

} // namespace Hirari::Core
