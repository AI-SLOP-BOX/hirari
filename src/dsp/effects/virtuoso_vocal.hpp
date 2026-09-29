#pragma once

#include "../../core/audio_buffer.hpp"
#include "../../core/midi_buffer.hpp"
#include "../../core/rust_ffi.hpp"
#include "../iprocessor.hpp"

namespace Hirari::DSP::Effects {

// Native processor facade; pitch shifting and formant filtering are Rust-owned.
class VirtuosoVocal final : public IProcessor {
public:
    explicit VirtuosoVocal(double sample_rate = 44100.0) noexcept
        : m_state(hirari_virtuoso_vocal_create(sample_rate)) {}

    ~VirtuosoVocal() override { hirari_virtuoso_vocal_destroy(m_state); }
    VirtuosoVocal(const VirtuosoVocal&) = delete;
    VirtuosoVocal& operator=(const VirtuosoVocal&) = delete;

    void prepareToPlay(double sample_rate, uint32_t) noexcept override {
        hirari_virtuoso_vocal_prepare(m_state, sample_rate);
    }

    std::string getName() const override { return "Virtuoso Vocal"; }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (m_bypassed || !m_state || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) {
            return;
        }
        hirari_virtuoso_vocal_process(
            m_state, buffer.getWritePointer(0),
            buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr,
            buffer.getNumSamples());
    }

    void reset() noexcept override { hirari_virtuoso_vocal_reset(m_state); }

    void setPitchShift(float semitones) noexcept {
        hirari_virtuoso_vocal_set_pitch(m_state, semitones);
    }

    void setFormantShift(float semitones) noexcept {
        hirari_virtuoso_vocal_set_formant(m_state, semitones);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
