#pragma once

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <vector>
#include "../midi_buffer.hpp"
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

// Compatibility facade. MPE state, articulation routing, and SysEx queue
// ownership all live in the Rust MIDI orchestration runtime.
class MIDIOrchestrator {
public:
    struct SysExBuffer {
        uint32_t manufacturerId = 0;
        std::vector<uint8_t> data;
    };

    struct ArticulationMap {
        uint32_t id = 0;
        char name[64]{};
        uint32_t triggerChannel = 0;
    };

    static_assert(sizeof(ArticulationMap) == 72);
    static_assert(offsetof(ArticulationMap, triggerChannel) == 68);

    MIDIOrchestrator() : m_state(hirari_mpe_state_create()) {}
    ~MIDIOrchestrator() { hirari_mpe_state_free(m_state); }

    MIDIOrchestrator(const MIDIOrchestrator&) = delete;
    MIDIOrchestrator& operator=(const MIDIOrchestrator&) = delete;

    void setMPEEnabled(bool enabled) noexcept {
        hirari_mpe_state_set_enabled(m_state, enabled);
    }

    void reset() noexcept { hirari_mpe_state_reset(m_state); }

    bool isMPEEnabled() const noexcept {
        return hirari_mpe_state_is_enabled(m_state);
    }

    void processMPE(MidiBuffer& buffer) noexcept {
        hirari_midi_process_mpe(m_state, buffer.getEvents(), buffer.size());
    }

    void setArticulationMap(const std::vector<ArticulationMap>& maps) noexcept {
        hirari_mpe_set_articulation_map(m_state, maps.data(), maps.size());
    }

    void sendSysEx(const SysExBuffer& message) noexcept {
        if (message.data.empty() || message.data.size() > 4096) return;
        (void)hirari_mpe_sysex_send(
            m_state, message.manufacturerId, message.data.data(), message.data.size());
    }

    void handleIncomingSysEx(const uint8_t* data, size_t size) noexcept {
        (void)hirari_mpe_sysex_incoming(m_state, data, size);
    }

    bool tryPopSysEx(SysExBuffer& message) noexcept {
        uint32_t manufacturer = 0;
        const size_t required = hirari_mpe_sysex_pop(
            m_state, &manufacturer, nullptr, 0);
        if (required == 0) return false;
        message.data.resize(required);
        const size_t copied = hirari_mpe_sysex_pop(
            m_state, &manufacturer, message.data.data(), message.data.size());
        if (copied == 0 || copied > required) {
            message.data.clear();
            return false;
        }
        message.data.resize(copied);
        message.manufacturerId = manufacturer;
        return true;
    }

    void processBlock(MidiBuffer& buffer) noexcept {
        processMPE(buffer);
        hirari_midi_apply_articulations_from_state(
            m_state, buffer.getMutableEvents(), buffer.size());
    }

private:
    HirariMpeState* m_state = nullptr;
};

} // namespace Hirari::Core::Engine
