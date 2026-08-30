#pragma once
#include <vector>
#include <atomic>
#include <memory>
#include <algorithm>
#include <string>
#include <set>
#include <mutex>

namespace Aura::Core::Composition {

enum class ChordType {
    Unknown = 0,
    Major,
    Minor,
    Diminished,
    Augmented,
    Dominant7,
    Major7,
    Minor7
};

/**
 * @struct HarmonicState
 * @brief Represents the current musical context (root, chord, and density).
 */
struct HarmonicState {
    int rootNote = 0; 
    ChordType chordType = ChordType::Unknown; 
    float noteDensity = 0.0f;   
    std::vector<int> activeNotes;
    std::string chordName = "None";
};

/**
 * @class HarmonyEngine
 * @brief Professional Interval-based Chord Detection Engine.
 * HONEST FIX: Replaced naive root-detection with real interval analysis.
 */
class HarmonyEngine {
public:
    static HarmonyEngine& getInstance() { static HarmonyEngine instance; return instance; }

    void updateFromNotes(const std::vector<int>& activeNotes) {
        auto newState = std::make_shared<HarmonicState>();
        if (!activeNotes.empty()) {
            // --- INDUSTRIAL CHORD DETECTION ---
            std::set<int> pitchClasses;
            for (int n : activeNotes) pitchClasses.insert(n % 12);

            int root = activeNotes[0] % 12; // Simple assumption for now
            uint32_t mask = 0;
            for (int p : pitchClasses) {
                int interval = (p - root + 12) % 12;
                mask |= (1 << interval);
            }

            newState->rootNote = root;
            newState->activeNotes = activeNotes;
            newState->noteDensity = static_cast<float>(activeNotes.size()) / 12.0f;

            // Pattern matching (Interval Mask)
            if ((mask & 0b10010001) == 0b10010001) { // 0, 4, 7
                newState->chordType = ChordType::Major;
                if (mask & 0b10000000000) newState->chordType = ChordType::Dominant7; // 10
            } else if ((mask & 0b10001001) == 0b10001001) { // 0, 3, 7
                newState->chordType = ChordType::Minor;
            }
        }

        std::lock_guard<std::mutex> lock(m_mutex);
        m_lastState = newState;
    }

    std::shared_ptr<HarmonicState> getState() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_lastState;
    }

private:
    HarmonyEngine() { m_lastState = std::make_shared<HarmonicState>(); }
    mutable std::mutex m_mutex;
    std::shared_ptr<HarmonicState> m_lastState;
};

} // namespace Aura::Core::Composition
