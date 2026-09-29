#pragma once

#include "../iprocessor.hpp"
#include "../../core/midi_buffer.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Synthesis {

/** The wavetable voice, envelopes, filters, and block renderer are Rust-owned. */
class HirariWavetableSynth final : public IProcessor {
public:
    explicit HirariWavetableSynth(double sampleRate = 44100.0)
        : m_state(hirari_hws_create(sampleRate)) {}
    ~HirariWavetableSynth() override { hirari_hws_destroy(m_state); }

    HirariWavetableSynth(const HirariWavetableSynth&) = delete;
    HirariWavetableSynth& operator=(const HirariWavetableSynth&) = delete;

    void prepareToPlay(double sampleRate, uint32_t) noexcept override {
        hirari_hws_prepare(m_state, sampleRate);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const ProcessContext&) noexcept override {
        if (buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        if (!left || !right) return;
        static_assert(sizeof(Core::MidiEvent) == 272);
        hirari_hws_process(m_state, midi.getEvents(), midi.size(), left, right,
                           buffer.getNumSamples());
    }

    void reset() noexcept override { hirari_hws_reset(m_state); }

    void noteOn(float frequency, float velocity) {
        hirari_hws_note_on(m_state, frequency, velocity);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Synthesis
