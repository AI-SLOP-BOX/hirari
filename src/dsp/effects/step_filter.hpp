#pragma once

#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** Rust-owned transport-synchronized step filter with a native processor adapter. */
class StepFilter final : public IProcessor {
public:
    StepFilter() : m_state(hirari_step_filter_create(44100.0)) {}
    ~StepFilter() override { hirari_step_filter_destroy(m_state); }

    StepFilter(const StepFilter&) = delete;
    StepFilter& operator=(const StepFilter&) = delete;

    void prepareToPlay(double sampleRate, uint32_t) noexcept override {
        hirari_step_filter_prepare(m_state, sampleRate);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext& context) noexcept override {
        if (buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        float* channelData[2] = {buffer.getWritePointer(0), nullptr};
        if (channels > 1) channelData[1] = buffer.getWritePointer(1);
        if (!channelData[0] || (channels > 1 && !channelData[1])) return;
        hirari_step_filter_process(m_state, channelData, channels, buffer.getNumSamples(),
                                   context.bpm, context.sampleRate, context.blockStart);
    }

    void reset() noexcept override { hirari_step_filter_reset(m_state); }
    void setStepValue(uint32_t step, float value) {
        hirari_step_filter_set_step(m_state, step, value);
    }
    void setResonance(float resonance) {
        hirari_step_filter_set_resonance(m_state, resonance);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
