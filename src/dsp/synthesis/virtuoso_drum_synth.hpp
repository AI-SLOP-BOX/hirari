#pragma once

#include "../../core/audio_buffer.hpp"
#include "../../core/midi_buffer.hpp"
#include "../../core/rust_ffi.hpp"
#include "../iprocessor.hpp"

namespace Hirari::DSP::Synthesis {

// Compatibility adapter: synthesis state and sample generation live in Rust.
class VirtuosoDrumSynth final : public IProcessor {
public:
    explicit VirtuosoDrumSynth(double sample_rate = 44100.0) noexcept
        : m_state(hirari_virtuoso_drum_synth_create(sample_rate)) {}

    ~VirtuosoDrumSynth() override {
        hirari_virtuoso_drum_synth_destroy(m_state);
    }

    VirtuosoDrumSynth(const VirtuosoDrumSynth&) = delete;
    VirtuosoDrumSynth& operator=(const VirtuosoDrumSynth&) = delete;

    void prepareToPlay(double sample_rate, uint32_t) noexcept override {
        hirari_virtuoso_drum_synth_prepare(m_state, sample_rate);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const ProcessContext&) noexcept override {
        if (!m_state || buffer.getNumChannels() < 2 || buffer.getNumSamples() == 0) {
            return;
        }
        hirari_virtuoso_drum_synth_process(
            m_state, midi.getEvents(), midi.size(), buffer.getWritePointer(0),
            buffer.getWritePointer(1), buffer.getNumSamples());
    }

    void reset() noexcept override {
        hirari_virtuoso_drum_synth_reset(m_state);
    }

    std::string getName() const override { return "Virtuoso Drum Synth"; }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Synthesis
