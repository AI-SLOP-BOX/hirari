#pragma once

#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** C++ plugin API adapter for the Rust real-time de-esser. */
class DeEsser final : public IProcessor {
public:
    DeEsser() : m_state(hirari_deesser_create(44'100.0)) {}
    ~DeEsser() override { hirari_deesser_destroy(m_state); }

    DeEsser(const DeEsser&) = delete;
    DeEsser& operator=(const DeEsser&) = delete;

    std::string getName() const override { return "DeEsser"; }
    uint32_t getTailSamples() const noexcept override {
        return hirari_deesser_tail_samples(m_state);
    }

    void prepareToPlay(double sampleRate, uint32_t) noexcept override {
        hirari_deesser_prepare(m_state, sampleRate);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        hirari_deesser_process(m_state, buffer.getWritePointer(0),
            buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr,
            buffer.getNumSamples());
    }

    void reset() noexcept override { hirari_deesser_reset(m_state); }

    void setThreshold(float threshold) noexcept {
        hirari_deesser_set_threshold(m_state, threshold);
    }
    void setIntensity(float intensity) noexcept {
        hirari_deesser_set_intensity(m_state, intensity);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
