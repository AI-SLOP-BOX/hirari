#pragma once

#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <cstring>
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
        const uint32_t magic = 0x41555241u;
        const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        const float values[] = {getParameter(0), getParameter(1), getParameter(2)};
        std::memcpy(state.data(), &magic, sizeof(magic));
        std::memcpy(state.data() + 4, &version, sizeof(version));
        std::memcpy(state.data() + 6, &flags, sizeof(flags));
        std::memcpy(state.data() + 8, &m_mix, sizeof(m_mix));
        std::memcpy(state.data() + 12, &m_sidechainBusId, sizeof(m_sidechainBusId));
        std::memcpy(state.data() + 16, values, sizeof(values));
        return state;
    }

    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 32) return false;
        uint32_t magic = 0, sidechain = 0;
        uint16_t version = 0, flags = 0;
        float mix = 0.0f, values[3]{};
        std::memcpy(&magic, state.data(), 4);
        std::memcpy(&version, state.data() + 4, 2);
        std::memcpy(&flags, state.data() + 6, 2);
        std::memcpy(&mix, state.data() + 8, 4);
        std::memcpy(&sidechain, state.data() + 12, 4);
        std::memcpy(values, state.data() + 16, sizeof(values));
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 ||
            !std::isfinite(mix) || mix < 0.0f || mix > 1.0f) return false;
        for (float value : values) {
            if (!std::isfinite(value) || value < 0.0f || value > 1.0f) return false;
        }
        m_bypassed = (flags & 1u) != 0;
        m_mix = mix;
        m_sidechainBusId = sidechain;
        for (uint32_t id = 0; id < 3; ++id) setParameter(id, values[id]);
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
