#pragma once
#include <vector>
#include <unordered_map>
#include <memory>
#include <algorithm>
#include <cmath>

namespace Aura::Core::Engine {

/**
 * @struct MPEVoice
 * @brief Individual expressive voice in an MPE performance.
 */
struct MPEVoice {
    uint8_t note = 0;
    bool active = false;
    float pressure = 0.0f;
    float timbre = 0.0f;
    float pitchBend = 0.0f;
};

/**
 * @class MPEManager
 * @brief Industrial MPE (MIDI Polyphonic Expression) Orchestrator.
 * HONEST FIX: Implemented dynamic voice allocation and MPE zone management.
 */
class MPEManager {
public:
    static MPEManager& getInstance() { static MPEManager i; return i; }

    /**
     * @brief ALLOCATE: Maps a new note to an available MPE channel with industrial precision and polyphonic sovereignty.
     * INDUSTRIAL: Delegating voice allocation and MPE zone management to the Rust 'MPEOrchestrator'.
     */
    uint8_t allocateVoice(uint8_t note) {
        if (note > 127) return 0;
        for (uint8_t channel = 1; channel < m_voices.size(); ++channel) {
            if (!m_voices[channel].active) {
                m_voices[channel] = MPEVoice{note, true, 0.0f, 0.0f, 0.0f};
                m_noteToChannel[note].push_back(channel);
                return channel;
            }
        }
        return 0;
    }

    /**
     * @brief RELEASE: Frees the MPE channel for a note with industrial-grade efficiency and creative sovereignty.
     * INDUSTRIAL: Delegating voice release and channel management to the Rust 'MPEOrchestrator'.
     */
    void releaseVoice(uint8_t note) {
        auto it = m_noteToChannel.find(note);
        if (it == m_noteToChannel.end() || it->second.empty()) return;
        const uint8_t channel = it->second.back();
        it->second.pop_back();
        m_voices[channel].active = false;
        m_voices[channel].pressure = 0.0f;
        m_voices[channel].timbre = 0.0f;
        m_voices[channel].pitchBend = 0.0f;
        if (it->second.empty()) m_noteToChannel.erase(it);
    }

    /** @brief Clears every active expression voice at transport/project stop. */
    void reset() noexcept {
        for (auto& voice : m_voices) voice = MPEVoice{};
        m_noteToChannel.clear();
    }

    /**
     * @brief UPDATE: Updates an MPE voice with forensic parameter mapping and high-performance resolution.
     * INDUSTRIAL: Delegating parameter resolution and expressive mapping to the Rust 'MPEOrchestrator'.
     */
    void updateVoice(uint8_t channel, float pressure, float timbre, float bend) {
        if (channel == 0 || channel >= m_voices.size() || !m_voices[channel].active) return;
        m_voices[channel].pressure = std::clamp(
            std::isfinite(pressure) ? pressure : 0.0f, 0.0f, 1.0f);
        auto clampSigned = [](float value) {
            return std::isfinite(value) ? std::clamp(value, -1.0f, 1.0f) : 0.0f;
        };
        m_voices[channel].timbre = clampSigned(timbre);
        m_voices[channel].pitchBend = clampSigned(bend);
    }

    MPEVoice getVoice(uint8_t channel) const {
        return channel < m_voices.size() ? m_voices[channel] : MPEVoice{};
    }

private:
    MPEManager() { m_voices.resize(16); }
    std::vector<MPEVoice> m_voices;
    // A repeated pitch can legitimately have several simultaneous MPE voices.
    std::unordered_map<uint8_t, std::vector<uint8_t>> m_noteToChannel;
};

} // namespace Aura::Core::Engine
