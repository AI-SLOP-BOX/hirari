#pragma once

#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** C++ processor adapter for the Rust-owned bitcrusher DSP state. */
class Bitcrusher final : public IProcessor {
public:
    Bitcrusher() : m_state(hirari_bitcrusher_create()) {}
    ~Bitcrusher() override { hirari_bitcrusher_destroy(m_state); }

    Bitcrusher(const Bitcrusher&) = delete;
    Bitcrusher& operator=(const Bitcrusher&) = delete;

    void prepareToPlay(double /*sampleRate*/, uint32_t /*blockSize*/) noexcept override {
        hirari_bitcrusher_reset(m_state);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        hirari_bitcrusher_process(
            m_state, buffer.getWritePointer(0),
            buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr,
            buffer.getNumSamples(), m_mix);
    }

    void reset() noexcept override { hirari_bitcrusher_reset(m_state); }

    void setBits(float bits) { hirari_bitcrusher_set_bits(m_state, bits); }
    void setDownsample(float downsample) {
        hirari_bitcrusher_set_downsample(m_state, downsample);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
