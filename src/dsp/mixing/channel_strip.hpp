#pragma once

#include <cstdint>
#include "../../core/audio_buffer.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Mixing {

/** Native buffer API façade for the Rust-owned channel strip processor. */
class ChannelStrip {
public:
    ChannelStrip() : m_state(hirari_channel_strip_create()) {}
    ~ChannelStrip() { hirari_channel_strip_destroy(m_state); }

    ChannelStrip(const ChannelStrip&) = delete;
    ChannelStrip& operator=(const ChannelStrip&) = delete;
    ChannelStrip(ChannelStrip&&) = delete;
    ChannelStrip& operator=(ChannelStrip&&) = delete;

    void setSampleRate(double sampleRate) {
        hirari_channel_strip_set_sample_rate(m_state, sampleRate);
    }

    double getSampleRate() const {
        return hirari_channel_strip_sample_rate(m_state);
    }

    void reset() { hirari_channel_strip_reset(m_state); }
    void setGain(float gain) { hirari_channel_strip_set_gain(m_state, gain); }
    void setPan(float pan) { hirari_channel_strip_set_pan(m_state, pan); }
    void setMute(bool mute) { hirari_channel_strip_set_mute(m_state, mute); }
    void setSolo(bool solo) { hirari_channel_strip_set_solo(m_state, solo); }

    void process(Core::AudioBuffer& buffer) {
        process(buffer, 0, buffer.getNumSamples());
    }

    void process(Core::AudioBuffer& buffer, uint32_t offset, uint32_t numSamples,
                 const float* /*extGain*/ = nullptr, const float* /*extPan*/ = nullptr) {
        if (buffer.getNumChannels() == 0 || numSamples == 0) return;
        hirari_channel_strip_process(m_state, buffer.getArrayOfWritePointers(),
                                     buffer.getNumChannels(), offset, numSamples);
    }

    void processWithMirror(Core::AudioBuffer& buffer, Core::AudioBuffer& mirror,
                           uint32_t numSamples) {
        if (numSamples == 0 || buffer.getNumChannels() == 0 ||
            buffer.getNumSamples() < numSamples) return;
        const uint32_t channels = buffer.getNumChannels();
        const bool hasMirror = mirror.getNumChannels() >= channels &&
                               mirror.getNumSamples() >= numSamples;
        float** mirrorChannels = hasMirror ? mirror.getArrayOfWritePointers() : nullptr;
        hirari_channel_strip_process_mirror(
            m_state, buffer.getArrayOfWritePointers(), channels,
            mirrorChannels, hasMirror ? mirror.getNumChannels() : 0, 0, numSamples);
    }

    const void* nativeState() const noexcept { return m_state; }

private:
    void* m_state;
};

} // namespace Hirari::DSP::Mixing
