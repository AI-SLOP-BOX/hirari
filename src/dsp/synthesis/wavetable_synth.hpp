#pragma once

#include "../iprocessor.hpp"
#include "../../core/midi_buffer.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Synthesis {

/** Rust-owned polyphonic wavetable synthesizer with a native processor adapter. */
class WavetableSynth final : public IProcessor {
public:
    WavetableSynth() : m_state(hirari_ws_create(44100.0)) {}
    ~WavetableSynth() override { hirari_ws_destroy(m_state); }

    WavetableSynth(const WavetableSynth&) = delete;
    WavetableSynth& operator=(const WavetableSynth&) = delete;

    void prepareToPlay(double sampleRate, uint32_t) noexcept override {
        hirari_ws_prepare(m_state, sampleRate);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        if (!left || !right) return;
        static_assert(sizeof(Core::MidiEvent) == 272);
        hirari_ws_process(m_state, midi.getEvents(), midi.size(), left, right,
                          buffer.getNumSamples());
    }

    void noteOn(uint8_t note, uint8_t velocity) { hirari_ws_note_on(m_state, note, velocity); }
    void noteOff(uint8_t note) { hirari_ws_note_off(m_state, note); }
    void reset() noexcept override { hirari_ws_reset(m_state); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Synthesis
