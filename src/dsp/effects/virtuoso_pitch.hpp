#pragma once

#include "../../core/audio_buffer.hpp"
#include "../../core/midi_buffer.hpp"
#include "../../core/rust_ffi.hpp"
#include "../iprocessor.hpp"

namespace Hirari::DSP::Effects {

// Native processor facade; pitch detection and shifting run in Rust.
class VirtuosoPitch final : public IProcessor {
public:
    explicit VirtuosoPitch(double sample_rate = 44100.0) noexcept
        : m_state(hirari_virtuoso_pitch_create(sample_rate)) {}

    ~VirtuosoPitch() override { hirari_virtuoso_pitch_destroy(m_state); }
    VirtuosoPitch(const VirtuosoPitch&) = delete;
    VirtuosoPitch& operator=(const VirtuosoPitch&) = delete;

    void prepareToPlay(double sample_rate, uint32_t) noexcept override {
        hirari_virtuoso_pitch_prepare(m_state, sample_rate);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (!m_state || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) {
            return;
        }
        hirari_virtuoso_pitch_process(
            m_state, buffer.getWritePointer(0),
            buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr,
            buffer.getNumSamples());
    }

    void reset() noexcept override { hirari_virtuoso_pitch_reset(m_state); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
