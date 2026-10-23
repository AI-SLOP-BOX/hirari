#pragma once

#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <vector>

#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** C++ host and session-state adapter for Rust tube saturation DSP. */
class TubeSaturation final : public IProcessor {
public:
    TubeSaturation() : m_state(hirari_tube_saturation_create()) {}
    ~TubeSaturation() override { hirari_tube_saturation_destroy(m_state); }

    TubeSaturation(const TubeSaturation&) = delete;
    TubeSaturation& operator=(const TubeSaturation&) = delete;

    std::string getName() const override { return "Tube Saturation"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }
    uint32_t getNumParameters() const noexcept override { return 3; }

    void setParameter(uint32_t id, float value) noexcept override {
        hirari_tube_saturation_set_parameter(m_state, id, value);
    }
    float getParameter(uint32_t id) const noexcept override {
        return hirari_tube_saturation_get_parameter(m_state, id);
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= getNumParameters()) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (outName && maxSize > 0) {
            std::snprintf(outName, maxSize, "%s",
                          id == 0 ? "Drive" : (id == 1 ? "Bias" : (id == 2 ? "Dry/Wet" : "")));
        }
    }

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(32, 0);
        if (hirari_tube_saturation_save_state(
                m_state, m_bypassed ? 1u : 0u, m_mix, m_sidechainBusId,
                state.data(), state.size()) != state.size()) state.clear();
        return state;
    }

    bool setState(const std::vector<uint8_t>& state) override {
        uint8_t bypassed = 0;
        float mix = 0.0f;
        uint32_t sidechain = 0;
        if (!hirari_tube_saturation_restore_state(
                m_state, state.data(), state.size(), &bypassed, &mix, &sidechain)) return false;
        m_bypassed = bypassed != 0;
        m_mix = mix;
        m_sidechainBusId = sidechain;
        return true;
    }
    bool restoreStateChecked(const std::vector<uint8_t>& state) override { return setState(state); }

    void prepareToPlay(double /*sampleRate*/, uint32_t /*blockSize*/) noexcept override { reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (isBypassed() || buffer.getNumChannels() == 0) return;
        float* left = buffer.getWritePointer(0);
        if (!left) return;
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        hirari_tube_saturation_process(m_state, left, right, buffer.getNumSamples());
    }
    void reset() noexcept override { hirari_tube_saturation_reset(m_state); }

    void setDrive(float db) { setControl(0, db); }
    void setBias(float bias) { setControl(1, bias); }
    void setDryWet(float mix) { setControl(2, mix); }

private:
    void setControl(uint32_t control, float value) {
        hirari_tube_saturation_set_control(m_state, control, value);
    }

    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
