#pragma once
#include <vector>
#include <cstdint>
#include <string>
#include <memory>
#include <algorithm>
#include <cmath>
#include <mutex>
#include "audio_types.hpp"
#include "midi_buffer.hpp"

namespace Aura::Core {

/**
 * @class MidiRegion
 * @brief Industrial MIDI Orchestrator for Aura Studio Pro.
 */
class MidiRegion {
public:
    MidiRegion(uint32_t id, const std::string& name, double startBeat = 0, double lengthBeats = 4.0) 
        : m_id(id), m_name(name), m_startBeat(startBeat), m_lengthBeats(lengthBeats) {}

    MidiRegion(const std::vector<MIDINote>& notes, double startBeat, double lengthBeats)
        : m_id(0), m_name("Generated"), m_startBeat(startBeat), m_lengthBeats(lengthBeats), m_notes(notes) {}

    uint32_t getId() const { return m_id; }
    const std::string& getName() const { return m_name; }

    void addNote(MIDINote n) {
        std::lock_guard<std::mutex> lock(m_notesMutex);
        m_notes.push_back(n);
    }
    bool removeNote(uint32_t index) {
        std::lock_guard<std::mutex> lock(m_notesMutex);
        if (index >= m_notes.size()) return false;
        m_notes.erase(m_notes.begin() + static_cast<std::ptrdiff_t>(index));
        return true;
    }
    void removeNotesAt(double beat, int pitch, double tolerance = 0.125) {
        std::lock_guard<std::mutex> lock(m_notesMutex);
        m_notes.erase(std::remove_if(m_notes.begin(), m_notes.end(),
            [&](const MIDINote& note) {
                return note.pitch == static_cast<uint8_t>(std::clamp(pitch, 0, 127)) &&
                       std::abs(note.startBeat - beat) <= std::max(0.0, tolerance);
            }), m_notes.end());
    }

    void setMutedAt(double beat, int pitch, bool muted) {
        std::lock_guard<std::mutex> lock(m_notesMutex);
        for (auto& note : m_notes) {
            if (note.pitch == static_cast<uint8_t>(std::clamp(pitch, 0, 127)) &&
                std::abs(note.startBeat - beat) <= 0.125) {
                note.velocity = muted ? 0 : std::max<uint8_t>(1, note.velocity);
            }
        }
    }
    [[deprecated("use copyProcessedNotes() for cross-thread reads")]]
    const std::vector<MIDINote>& getProcessedNotes() const { return m_notes; }
    // Legacy reference access is retained for control-thread callers. New
    // readers must use copyProcessedNotes() so UI/CLI edits cannot race a
    // serializer or renderer.
    bool copyProcessedNotes(std::vector<MIDINote>& destination) const {
        std::lock_guard<std::mutex> lock(m_notesMutex);
        destination = m_notes;
        return true;
    }

    bool updateNote(size_t index, const MIDINote& note) {
        if (!std::isfinite(note.startBeat) || !std::isfinite(note.lengthBeats) ||
            note.startBeat < 0.0 || note.lengthBeats <= 0.0) return false;
        std::lock_guard<std::mutex> lock(m_notesMutex);
        if (index >= m_notes.size()) return false;
        m_notes[index] = note;
        return true;
    }

    bool replaceNotes(const std::vector<MIDINote>& notes) {
        if (notes.size() > 1'000'000) return false;
        for (const auto& note : notes) {
            if (!std::isfinite(note.startBeat) || !std::isfinite(note.lengthBeats) ||
                note.startBeat < 0.0 || note.lengthBeats <= 0.0) return false;
        }
        std::lock_guard<std::mutex> lock(m_notesMutex);
        m_notes = notes;
        return true;
    }

    void transpose(int semitones, double startBeat = -1.0, double endBeat = -1.0) {
        std::lock_guard<std::mutex> lock(m_notesMutex);
        for (auto& note : m_notes) {
            if (startBeat >= 0.0 && (note.startBeat < startBeat ||
                (endBeat >= 0.0 && note.startBeat > endBeat))) continue;
            const int pitch = std::clamp(static_cast<int>(note.pitch) + semitones, 0, 127);
            note.pitch = static_cast<uint8_t>(pitch);
        }
    }

    void quantize(double grid, double strength = 1.0, double startBeat = -1.0, double endBeat = -1.0) {
        if (!std::isfinite(grid) || grid <= 0.0 || !std::isfinite(strength)) return;
        strength = std::clamp(strength, 0.0, 1.0);
        std::lock_guard<std::mutex> lock(m_notesMutex);
        for (auto& note : m_notes) {
            if (startBeat >= 0.0 && (note.startBeat < startBeat ||
                (endBeat >= 0.0 && note.startBeat > endBeat))) continue;
            const double snapped = std::round(note.startBeat / grid) * grid;
            note.startBeat += (snapped - note.startBeat) * strength;
        }
        std::stable_sort(m_notes.begin(), m_notes.end(),
            [](const MIDINote& a, const MIDINote& b) { return a.startBeat < b.startBeat; });
    }
    
    double getStartBeat() const { return m_startBeat; }
    double getLengthBeats() const { return m_lengthBeats; }

private:
    uint32_t m_id;
    std::string m_name;
    double m_startBeat;
    double m_lengthBeats;
    std::vector<MIDINote> m_notes;
    mutable std::mutex m_notesMutex;
};

using MIDIRegion = MidiRegion;

} // namespace Aura::Core
