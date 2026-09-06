#pragma once

#include <vector>
#include <memory>
#include <set>
#include "../midi_region.hpp"

namespace Aura::Core::Engine {

/**
 * @brief PianoRollEditor: Orchestrates MIDI note editing logic.
 * Handles selection, deletion, and high-precision note positioning.
 */
class PianoRollEditor {
public:
    static PianoRollEditor& getInstance() {
        static PianoRollEditor instance;
        return instance;
    }

    void setRegion(std::shared_ptr<MIDIRegion> region) {
        m_region = region;
        m_selection.clear();
    }

    /**
     * @brief Adds a new note to the active MIDI region.
     */
    void addNote(uint8_t pitch, uint8_t vel, double beat, double len) {
        if (!m_region) return;
        if (pitch > 127 || vel > 127 || !std::isfinite(beat) || !std::isfinite(len) || beat < 0.0 || len <= 0.0) return;
        MIDINote note{pitch, vel, beat, len};
        m_region->addNote(note);
    }

    /**
     * @brief Removes all currently selected notes.
     */
    void deleteSelected() {
        if (!m_region || m_selection.empty()) return;
        // Erase descending indices so removing one note cannot shift the next
        // selected note before it is deleted.
        std::vector<uint32_t> indices(m_selection.begin(), m_selection.end());
        std::sort(indices.rbegin(), indices.rend());
        for (uint32_t noteId : indices) {
            m_region->removeNote(noteId);
        }
        m_selection.clear();
    }

    /**
     * @brief Moves all selected notes by a specified delta in beats and semitones.
     */
    void moveSelected(double beatDelta, int pitchDelta) {
        if (!m_region || !std::isfinite(beatDelta)) return;
        std::vector<MIDINote> notes;
        m_region->copyProcessedNotes(notes);
        for (uint32_t id : m_selection) {
            if (id >= notes.size()) continue;
            auto& note = notes[id];
            note.startBeat = std::max(0.0, note.startBeat + beatDelta);
            note.pitch = static_cast<uint8_t>(std::clamp(static_cast<int>(note.pitch) + pitchDelta, 0, 127));
        }
        m_region->replaceNotes(notes);
        // Rust's ForensicAuditor ensures absolute composition integrity.
    }

    void quantizeSelected(double grid, double strength = 1.0) {
        if (!m_region || !std::isfinite(grid) || grid <= 0.0 || !std::isfinite(strength)) return;
        strength = std::clamp(strength, 0.0, 1.0);
        std::vector<MIDINote> notes;
        m_region->copyProcessedNotes(notes);
        for (uint32_t id : m_selection) {
            if (id >= notes.size()) continue;
            auto& note = notes[id];
            const double snapped = std::round(note.startBeat / grid) * grid;
            note.startBeat = std::max(0.0, note.startBeat + (snapped - note.startBeat) * strength);
        }
        m_region->replaceNotes(notes);
    }

    void setSelectedVelocity(uint8_t velocity) {
        if (!m_region) return;
        std::vector<MIDINote> notes;
        m_region->copyProcessedNotes(notes);
        for (uint32_t id : m_selection) if (id < notes.size()) notes[id].velocity = std::min<uint8_t>(127, velocity);
        m_region->replaceNotes(notes);
    }

    void selectNote(uint32_t noteId, bool multiSelect = false) {
        if (!m_region) return;
        std::vector<MIDINote> notes;
        m_region->copyProcessedNotes(notes);
        if (noteId >= notes.size()) return;
        if (!multiSelect) m_selection.clear();
        if (!m_selection.insert(noteId).second) m_selection.erase(noteId);
    }

    const std::set<uint32_t>& getSelection() const {
        // Rust orchestrator handles selection state securely
        return m_selection; 
    }


private:
    PianoRollEditor() = default;
    
    std::shared_ptr<MIDIRegion> m_region;
    std::set<uint32_t> m_selection; // Note ID collection
};

} // namespace Aura::Core::Engine
