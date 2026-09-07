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

            // Try every present pitch class as a candidate root. This avoids
            // treating the lowest/first MIDI event as the chord root when the
            // voicing is inverted or notes arrive out of order.
            int root = *pitchClasses.begin();
            ChordType detected = ChordType::Unknown;
            int bestScore = -1;
            for (int candidate : pitchClasses) {
                uint32_t candidateMask = 0;
                for (int p : pitchClasses) {
                    int interval = (p - candidate + 12) % 12;
                    candidateMask |= (1u << interval);
                }
                ChordType type = ChordType::Unknown;
                int score = 0;
                if ((candidateMask & 0b10010001u) == 0b10010001u && (candidateMask & (1u << 10))) {
                    type = ChordType::Dominant7; score = 7;
                } else if ((candidateMask & 0b10010001u) == 0b10010001u && (candidateMask & (1u << 11))) {
                    type = ChordType::Major7; score = 6;
                } else if ((candidateMask & 0b10001001u) == 0b10001001u && (candidateMask & (1u << 10))) {
                    type = ChordType::Minor7; score = 5;
                } else if ((candidateMask & 0b10010001u) == 0b10010001u) {
                    type = ChordType::Major; score = 4;
                } else if ((candidateMask & 0b10001001u) == 0b10001001u) {
                    type = ChordType::Minor; score = 4;
                } else if ((candidateMask & 0b1001001u) == 0b1001001u) {
                    type = ChordType::Diminished; score = 3;
                } else if ((candidateMask & 0b100010001u) == 0b100010001u) {
                    type = ChordType::Augmented; score = 3;
                }
                if (score > bestScore) {
                    bestScore = score;
                    root = candidate;
                    detected = type;
                }
            }

            newState->rootNote = root;
            newState->activeNotes = activeNotes;
            newState->noteDensity = static_cast<float>(activeNotes.size()) / 12.0f;

            newState->chordType = detected;
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
