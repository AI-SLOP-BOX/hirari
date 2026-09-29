#pragma once

#include <cstdint>
#include "../../core/audio_buffer.hpp"
#include "../../core/midi_buffer.hpp"
#include "../../core/engine/macro_control_manager.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Instruments {

/** C++ MIDI/control adapter for the Rust wavetable synth. */
class NeuralSynth {
public:
    NeuralSynth() : m_state(hirari_neural_synth_create()) {}
    ~NeuralSynth() { hirari_neural_synth_destroy(m_state); }
    NeuralSynth(const NeuralSynth&) = delete;
    NeuralSynth& operator=(const NeuralSynth&) = delete;
    NeuralSynth(NeuralSynth&&) = delete;
    NeuralSynth& operator=(NeuralSynth&&) = delete;

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, double sampleRate) {
        if (!m_state || buffer.isEmpty() || buffer.getNumChannels() == 0) return;
        for (const auto& event : midi) {
            if (event.size < 2 || event.data[0] < 0x80u) continue;
            const uint8_t status = event.data[0] & 0xF0u;
            if (status == 0x90u && event.size >= 3 && event.data[2] != 0) {
                hirari_neural_synth_note_on(m_state, event.data[1], event.data[2]);
            } else if (status == 0x80u || (status == 0x90u && event.size >= 3 && event.data[2] == 0)) {
                hirari_neural_synth_note_off(m_state, event.data[1]);
            }
        }
        const float morph = Core::Engine::MacroControlManager::getInstance().getMacroValue(0);
        hirari_neural_synth_process(m_state, buffer.getWritePointer(0),
            buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr,
            buffer.getNumSamples(), sampleRate, morph);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Instruments
