#pragma once

#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** C++ processor API adapter for Rust transient-envelope shaping. */
class TransientShaper final : public IProcessor {
public:
    TransientShaper() : m_state(hirari_transient_shaper_create(44'100.0)) {}
    ~TransientShaper() override { hirari_transient_shaper_destroy(m_state); }

    TransientShaper(const TransientShaper&) = delete;
    TransientShaper& operator=(const TransientShaper&) = delete;

    std::string getName() const override { return "Transient Shaper"; }
    void prepareToPlay(double sampleRate, uint32_t) noexcept override {
        hirari_transient_shaper_prepare(m_state, sampleRate);
    }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        hirari_transient_shaper_process(m_state, buffer.getWritePointer(0),
            buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr,
            buffer.getNumSamples());
    }
    void reset() noexcept override { hirari_transient_shaper_reset(m_state); }

    void setAttack(float attack) noexcept {
        hirari_transient_shaper_set_attack(m_state, attack);
    }
    void setSustain(float sustain) noexcept {
        hirari_transient_shaper_set_sustain(m_state, sustain);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
