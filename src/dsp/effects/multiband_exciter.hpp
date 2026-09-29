#pragma once

#include <algorithm>
#include "../iprocessor.hpp"

namespace Hirari::DSP::Effects {

/** C++ host adapter for the Rust-owned multiband exciter DSP. */
class MultibandExciter : public IProcessor {
public:
    explicit MultibandExciter(double sampleRate = 44100.0)
        : m_rustEngine(hirari_multiband_exciter_create(sampleRate)) {}

    ~MultibandExciter() override { hirari_multiband_exciter_destroy(m_rustEngine); }
    MultibandExciter(const MultibandExciter&) = delete;
    MultibandExciter& operator=(const MultibandExciter&) = delete;

    void setupCrossover(float lowCut, float highCut) {
        hirari_multiband_exciter_setup_crossover(m_rustEngine, lowCut, highCut);
    }

    void process(float* left, float* right, uint32_t samples) {
        hirari_multiband_exciter_process(m_rustEngine, left, right, samples);
    }

    void prepareToPlay(double sampleRate, uint32_t blockSize) noexcept override {
        (void)blockSize;
        setSampleRate(sampleRate);
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        process(left, right, buffer.getNumSamples());
    }

    void reset() noexcept override { hirari_multiband_exciter_reset(m_rustEngine); }

    void setSampleRate(double sampleRate) {
        hirari_multiband_exciter_set_sample_rate(m_rustEngine, sampleRate);
        setupCrossover(200.0f, 3000.0f);
    }

    uint32_t getLatency() const { return 0; }

private:
    void* m_rustEngine;
};

} // namespace Hirari::DSP::Effects
