#pragma once
#include <map>
#include <cstdint>

namespace Aura::Core::Midi {

/**
 * @struct NoteExpressionState
 * @brief High-resolution per-note expression state.
 */
struct NoteExpressionState {
    uint16_t pitchBend;
    uint16_t pressure;
    uint16_t timbre;
};

/**
 * @class PerNoteExpressionManager
 * @brief Orchestrates MPE+ expression data.
 */
class PerNoteExpressionManager {
public:
    static PerNoteExpressionManager& getInstance() {
        static PerNoteExpressionManager instance;
        return instance;
    }

    /**
     * @brief Updates expression state for a specific note.
     */
    void updateNoteExpression(uint8_t noteLine, uint16_t pressure, uint16_t pitch) {
        m_noteStates[noteLine] = {pitch, pressure, 0};
    }

private:
    PerNoteExpressionManager() = default;
    std::map<uint8_t, NoteExpressionState> m_noteStates;
};

} // namespace Aura::Core::Midi
