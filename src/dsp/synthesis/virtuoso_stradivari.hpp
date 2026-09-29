#pragma once

#include "../../core/audio_buffer.hpp"
#include "../../core/midi_buffer.hpp"
#include "../../core/rust_ffi.hpp"
#include "../iprocessor.hpp"

namespace Hirari::DSP::Synthesis {

// Native host adapter; polyphonic waveguide synthesis is implemented in Rust.
class VirtuosoStradivari final : public IProcessor {
public:
    static constexpr int kMaxPolyphony = 8;
    static constexpr int kDelayBufferSize = 4096;
    static constexpr int kDelayMask = kDelayBufferSize - 1;

    explicit VirtuosoStradivari(double sample_rate = 44100.0) noexcept
        : m_state(hirari_virtuoso_stradivari_create(sample_rate)) {}

    ~VirtuosoStradivari() override { hirari_virtuoso_stradivari_destroy(m_state); }
    VirtuosoStradivari(const VirtuosoStradivari&) = delete;
    VirtuosoStradivari& operator=(const VirtuosoStradivari&) = delete;

    void prepareToPlay(double sample_rate, uint32_t) noexcept override {
        hirari_virtuoso_stradivari_prepare(m_state, sample_rate);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const ProcessContext&) noexcept override {
        if (!m_state || buffer.getNumChannels() < 2 || buffer.getNumSamples() == 0) {
            return;
        }
        hirari_virtuoso_stradivari_process(
            m_state, midi.getEvents(), midi.size(), buffer.getWritePointer(0),
            buffer.getWritePointer(1), buffer.getNumSamples());
    }

    void reset() noexcept override { hirari_virtuoso_stradivari_reset(m_state); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Synthesis
