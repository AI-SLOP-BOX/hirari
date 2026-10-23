#pragma once

#include <cstdint>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

struct MPEVoice {
    uint8_t note = 0;
    bool active = false;
    float pressure = 0.0f;
    float timbre = 0.0f;
    float pitchBend = 0.0f;
};

/** Native singleton adapter for Rust MPE channel and voice management. */
class MPEManager {
public:
    static MPEManager& getInstance() { static MPEManager instance; return instance; }
    ~MPEManager() { hirari_mpe_manager_destroy(m_state); }
    MPEManager(const MPEManager& other) : m_state(hirari_mpe_manager_clone(other.m_state)) {}
    MPEManager& operator=(const MPEManager& other) {
        if (this == &other) return *this;
        void* replacement = hirari_mpe_manager_clone(other.m_state);
        if (replacement) {
            hirari_mpe_manager_destroy(m_state);
            m_state = replacement;
        }
        return *this;
    }

    uint8_t allocateVoice(uint8_t note) { return hirari_mpe_manager_allocate(m_state, note); }
    void releaseVoice(uint8_t note) { hirari_mpe_manager_release(m_state, note); }
    void reset() noexcept { hirari_mpe_manager_reset(m_state); }
    void updateVoice(uint8_t channel, float pressure, float timbre, float bend) {
        hirari_mpe_manager_update(m_state, channel, pressure, timbre, bend);
    }
    MPEVoice getVoice(uint8_t channel) const {
        MPEVoice voice{};
        hirari_mpe_manager_get(m_state, channel, &voice.note, &voice.active,
                               &voice.pressure, &voice.timbre, &voice.pitchBend);
        return voice;
    }

private:
    MPEManager() : m_state(hirari_mpe_manager_create()) {}
    void* m_state = nullptr;
};

} // namespace Hirari::Core::Engine
