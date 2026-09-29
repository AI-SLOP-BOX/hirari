#pragma once

#include <cstdint>
#include "rust_ffi.hpp"

namespace Hirari::Core {

/** Rust-owned MIDI CC and 14-bit CC mapping state. */
class MidiLearnManager {
public:
    MidiLearnManager() noexcept : m_state(hirari_midi_learn_create()) {}
    ~MidiLearnManager() { hirari_midi_learn_destroy(m_state); }
    MidiLearnManager(const MidiLearnManager&) = delete;
    MidiLearnManager& operator=(const MidiLearnManager&) = delete;

    void addMapping(uint8_t cc, const void* macroState, uint32_t macroIndex) {
        addMappingAdvanced(cc, 16, macroState, macroIndex, 0.0f, 1.0f, 0.0f, false);
    }

    void addMappingAdvanced(uint8_t cc, uint8_t channel, const void* macroState,
                            uint32_t macroIndex,
                            float minimum, float maximum, float curve, bool pickup) {
        hirari_midi_learn_add_mapping(
            m_state, cc, channel, macroState, macroIndex,
            minimum, maximum, curve, pickup);
    }

    void addMapping14Bit(uint16_t controller, uint8_t channel,
                         const void* macroState, uint32_t macroIndex, float minimum,
                         float maximum, float curve, bool pickup) {
        hirari_midi_learn_add_mapping_14bit(
            m_state, controller, channel, macroState, macroIndex,
            minimum, maximum, curve, pickup);
    }

    void removeMapping(uint8_t cc, uint8_t channel = 16) {
        hirari_midi_learn_remove_mapping(m_state, cc, channel);
    }

    void handleMidiCC(uint8_t channel, uint8_t cc, uint8_t value) noexcept {
        hirari_midi_learn_handle_cc(m_state, channel, cc, value);
    }

    void handleMidiCC14(uint8_t channel, uint16_t controller, uint16_t value) noexcept {
        hirari_midi_learn_handle_cc14(m_state, channel, controller, value);
    }

    // Compatibility form for older device bridges without channel metadata.
    void handleMidiCC(uint8_t cc, uint8_t value) noexcept { handleMidiCC(0, cc, value); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core
