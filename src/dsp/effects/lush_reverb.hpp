#pragma once

#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** C++ host adapter for the Rust-owned Lush Reverb DSP. */
class LushReverb final : public IProcessor {
public:
    explicit LushReverb(double sampleRate = 44'100.0)
        : m_state(hirari_lush_reverb_create(sampleRate)) {}
    ~LushReverb() override { hirari_lush_reverb_destroy(m_state); }

    LushReverb(const LushReverb&) = delete;
    LushReverb& operator=(const LushReverb&) = delete;
    LushReverb(LushReverb&&) = delete;
    LushReverb& operator=(LushReverb&&) = delete;

    std::string getName() const override { return "Lush Reverb"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }
    uint32_t getTailSamples() const noexcept override { return hirari_lush_reverb_tail(m_state); }

    void setSampleRate(double sampleRate) {
        hirari_lush_reverb_set_sample_rate(m_state, sampleRate);
    }

    void process(float* left, float* right, uint32_t samples) noexcept {
        if (left && right && samples != 0)
            hirari_lush_reverb_process(m_state, left, right, samples);
    }

    void prepareToPlay(double sampleRate, uint32_t /*blockSize*/) noexcept override {
        setSampleRate(sampleRate);
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        process(left, right, buffer.getNumSamples());
    }

    void reset() noexcept override { hirari_lush_reverb_reset(m_state); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
