#pragma once

#include <algorithm>
#include <array>
#include <vector>
#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** C++ processor API adapter for the Rust 12-channel FDN reverb. */
class AtmosReverb final : public IProcessor {
public:
    explicit AtmosReverb(double sampleRate = 44'100.0)
        : m_state(hirari_atmos_reverb_create(sampleRate)) {}
    ~AtmosReverb() override { hirari_atmos_reverb_destroy(m_state); }

    AtmosReverb(const AtmosReverb&) = delete;
    AtmosReverb& operator=(const AtmosReverb&) = delete;

    std::string getName() const override { return "Atmos Immersive Reverb"; }

    void processImmersiveRaw(float* const* buffers, uint32_t bufferCount,
                             uint32_t numSamples) noexcept {
        hirari_atmos_reverb_process(m_state, buffers, bufferCount, numSamples);
    }

    void processImmersive(const std::vector<float*>& buffers, uint32_t numSamples) noexcept {
        processImmersiveRaw(buffers.data(), static_cast<uint32_t>(buffers.size()), numSamples);
    }

    void process(float* left, float* right, uint32_t numSamples) noexcept {
        if (!left || !right) return;
        m_bufferPointers.fill(nullptr);
        m_bufferPointers[0] = left;
        m_bufferPointers[1] = right;
        processImmersiveRaw(m_bufferPointers.data(), 2, numSamples);
    }

    void prepareToPlay(double sampleRate, uint32_t) noexcept override {
        hirari_atmos_reverb_prepare(m_state, sampleRate);
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 12);
        for (uint32_t channel = 0; channel < 12; ++channel) {
            m_bufferPointers[channel] = channel < channels
                ? buffer.getWritePointer(channel) : nullptr;
        }
        processImmersiveRaw(m_bufferPointers.data(), channels, buffer.getNumSamples());
    }

    void reset() noexcept override { hirari_atmos_reverb_reset(m_state); }
    uint32_t getLatencySamples() const noexcept override { return 0; }
    uint32_t getTailSamples() const noexcept override {
        return hirari_atmos_reverb_tail_samples(m_state);
    }
    void setSampleRate(double sampleRate) noexcept {
        hirari_atmos_reverb_prepare(m_state, sampleRate);
    }

private:
    void* m_state = nullptr;
    std::array<float*, 12> m_bufferPointers{};
};

} // namespace Hirari::DSP::Effects
