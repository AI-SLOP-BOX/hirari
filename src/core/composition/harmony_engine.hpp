#pragma once
#include <vector>
#include <atomic>
#include <memory>
#include <algorithm>
#include <string>
#include <mutex>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Composition {

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
            int root = 0;
            uint8_t detected = 0;
            if (!hirari_harmony_detect(activeNotes.data(), activeNotes.size(), &root, &detected))
                return;
            newState->rootNote = root;
            newState->activeNotes = activeNotes;
            newState->noteDensity = static_cast<float>(activeNotes.size()) / 12.0f;
            newState->chordType = static_cast<ChordType>(detected);
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

} // namespace Hirari::Core::Composition
