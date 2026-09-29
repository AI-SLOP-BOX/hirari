#pragma once

#include <algorithm>
#include <cstdio>

#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** Host adapter for the Rust sub-frequency tracker and synthesizer. */
class SubBassGenerator final : public IProcessor {
public:
    explicit SubBassGenerator(double sampleRate = 44'100.0)
        : m_state(hirari_sub_bass_create(sampleRate)) {
        setMix(0.5f);
    }
    ~SubBassGenerator() override { hirari_sub_bass_destroy(m_state); }

    SubBassGenerator(const SubBassGenerator&) = delete;
    SubBassGenerator& operator=(const SubBassGenerator&) = delete;

    void prepareToPlay(double sampleRate, uint32_t /*blockSize*/) noexcept override {
        hirari_sub_bass_prepare(m_state, sampleRate);
    }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (isBypassed() || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        float* left = buffer.getWritePointer(0);
        if (!left) return;
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        hirari_sub_bass_set_parameter(m_state, 0, m_mix);
        hirari_sub_bass_process(m_state, left, right, buffer.getNumSamples());
    }
    void reset() noexcept override { hirari_sub_bass_reset(m_state); }

    void setParameter(uint32_t id, float value) noexcept override {
        hirari_sub_bass_set_parameter(m_state, id, value);
        if (id == 0) setMix(std::isfinite(value) ? value : 0.0f);
    }
    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return m_mix;
        return hirari_sub_bass_get_parameter(m_state, id);
    }
    uint32_t getNumParameters() const noexcept override { return 1; }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id != 0) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (outName && maxSize > 0) std::snprintf(outName, maxSize, "%s", id == 0 ? "Sub Amount" : "");
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (!IProcessor::setState(state)) return false;
        hirari_sub_bass_set_parameter(m_state, 0, m_mix);
        return true;
    }
    bool restoreStateChecked(const std::vector<uint8_t>& state) override { return setState(state); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
