#pragma once

#include <array>
#include <algorithm>
#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** C++ host adapter for the Rust-owned three-band compressor. */
class MultibandCompressor final : public IProcessor {
public:
    explicit MultibandCompressor(double sampleRate = 44'100.0)
        : m_state(hirari_multiband_compressor_create(sampleRate)) {}
    ~MultibandCompressor() override { hirari_multiband_compressor_destroy(m_state); }

    MultibandCompressor(const MultibandCompressor&) = delete;
    MultibandCompressor& operator=(const MultibandCompressor&) = delete;
    MultibandCompressor(MultibandCompressor&&) = delete;
    MultibandCompressor& operator=(MultibandCompressor&&) = delete;

    std::string getName() const override { return "Multiband Compressor"; }
    uint32_t getTailSamples() const noexcept override {
        return hirari_multiband_compressor_tail(m_state);
    }

    void setSplitFreqs(float lowMid, float midHigh) {
        hirari_multiband_compressor_set_split_freqs(m_state, lowMid, midHigh);
    }

    void prepareToPlay(double sampleRate, uint32_t /*blockSize*/) noexcept override {
        hirari_multiband_compressor_prepare(m_state, sampleRate);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        std::array<float*, 2> channels{};
        const uint32_t count = std::min<uint32_t>(buffer.getNumChannels(), channels.size());
        for (uint32_t channel = 0; channel < count; ++channel)
            channels[channel] = buffer.getWritePointer(channel);
        hirari_multiband_compressor_process(
            m_state, channels.data(), count, buffer.getNumSamples());
    }

    void reset() noexcept override { hirari_multiband_compressor_reset(m_state); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
