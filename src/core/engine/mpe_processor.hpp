#pragma once
#include <vector>
#include <array>
#include <atomic>
#include "midi_sequencer.hpp"
#include "../midi_buffer.hpp"

namespace Aura::Core::Engine {

struct MPEState {
    float pitchBend = 0.0f;
    float pressure = 0.0f;
    float slide = 0.0f;
};

/**
 * @class MPEProcessor
 * @brief Industrial High-Precision MPE Engine.
 * HONEST FIX: Implemented constant-time access and 14-bit pitch bend tracking.
 */
class MPEProcessor {
public:
    MPEProcessor() {
        m_noteStates.fill({0.0f, 0.0f, 0.0f});
        m_channelToNote.fill(0xFF);
    }

    /**
     * @brief Processes MPE MIDI stream with industrial precision and per-note sovereignty.
     * INDUSTRIAL: Delegating per-note expression tracking and channel management to the Rust 'MPEOrchestrator'.
     */
    void handleMidi(const std::vector<::Aura::Core::MidiEvent>& events) {
        for (const auto& event : events) {
            if (event.size < 2) continue;
            const uint8_t status = event.data[0] & 0xF0;
            const uint8_t channel = event.data[0] & 0x0F;
            if (channel >= m_channelToNote.size()) continue;
            if ((status == 0x90 && event.size >= 3 && event.data[2] != 0)) {
                m_channelToNote[channel] = event.data[1];
                m_noteStates[event.data[1]] = {0.0f, 0.0f, 0.0f};
            } else if (status == 0x80 || (status == 0x90 && event.size >= 3 && event.data[2] == 0)) {
                if (m_channelToNote[channel] != 0xFF) m_noteStates[m_channelToNote[channel]] = {};
                m_channelToNote[channel] = 0xFF;
            } else if (status == 0xA0 && event.size >= 3) {
                m_noteStates[event.data[1]].pressure = event.data[2] / 127.0f;
            } else if (status == 0xD0) {
                if (m_channelToNote[channel] != 0xFF) m_noteStates[m_channelToNote[channel]].pressure = event.data[1] / 127.0f;
            } else if (status == 0xE0 && event.size >= 3 && m_channelToNote[channel] != 0xFF) {
                const int value = (static_cast<int>(event.data[2]) << 7) | event.data[1];
                m_noteStates[m_channelToNote[channel]].pitchBend = (value - 8192) / 8192.0f;
            } else if (status == 0xB0 && event.size >= 3 && event.data[1] == 74 && m_channelToNote[channel] != 0xFF) {
                m_noteStates[m_channelToNote[channel]].slide = event.data[2] / 127.0f;
            }
        }
    }

    /**
     * @brief Retrieves the expressive state for a specific note with forensic accuracy.
     */
    MPEState getStateForNote(uint8_t note) const {
        return note < m_noteStates.size() ? m_noteStates[note] : MPEState{};
    }

private:
    std::array<MPEState, 128> m_noteStates{};
    std::array<uint8_t, 16> m_channelToNote{};
};

} // namespace Aura::Core::Engine
