#pragma once

#include <cmath>
#include <cstdio>
#include <cstring>
#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** C++ host, automation, and persistence adapter for Rust StereoExpanderEngine. */
class StereoExpander final : public IProcessor {
public:
    StereoExpander() : m_state(hirari_stereo_expander_create()) {}
    ~StereoExpander() override { hirari_stereo_expander_destroy(m_state); }

    StereoExpander(const StereoExpander&) = delete;
    StereoExpander& operator=(const StereoExpander&) = delete;

    std::string getName() const override { return "Stereo Expander"; }
    uint32_t getNumParameters() const noexcept override { return 2; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        if (id == 0) setWidth(value * 2.0f);
        else if (id == 1) setMidGain(value * 2.0f);
    }
    float getParameter(uint32_t id) const noexcept override {
        return id < 2 ? hirari_stereo_expander_get_parameter(m_state, id) * 0.5f : 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 2) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (outName && maxSize > 0) {
            std::snprintf(outName, maxSize, "%s", id == 0 ? "Width" : (id == 1 ? "Mid Gain" : ""));
        }
    }

    void setWidth(float width) noexcept {
        hirari_stereo_expander_set_parameter(m_state, 0, width);
    }
    void setMidGain(float gain) noexcept {
        hirari_stereo_expander_set_parameter(m_state, 1, gain);
    }

    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumChannels() < 2 || buffer.getNumSamples() == 0) return;
        hirari_stereo_expander_process(m_state, buffer.getWritePointer(0),
            buffer.getWritePointer(1), buffer.getNumSamples(), m_mix);
    }
    void reset() noexcept override {}

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(24, 0);
        const uint32_t magic = 0x41555241u;
        const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        const float values[] = {getParameter(0), getParameter(1)};
        std::memcpy(state.data(), &magic, 4);
        std::memcpy(state.data() + 4, &version, 2);
        std::memcpy(state.data() + 6, &flags, 2);
        std::memcpy(state.data() + 8, &m_mix, 4);
        std::memcpy(state.data() + 12, &m_sidechainBusId, 4);
        std::memcpy(state.data() + 16, values, sizeof(values));
        return state;
    }

    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 24) return false;
        uint32_t magic = 0, sidechain = 0;
        uint16_t version = 0, flags = 0;
        float mix = 0.0f, values[2]{};
        std::memcpy(&magic, state.data(), 4);
        std::memcpy(&version, state.data() + 4, 2);
        std::memcpy(&flags, state.data() + 6, 2);
        std::memcpy(&mix, state.data() + 8, 4);
        std::memcpy(&sidechain, state.data() + 12, 4);
        std::memcpy(values, state.data() + 16, sizeof(values));
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 ||
            !std::isfinite(mix) || mix < 0.0f || mix > 1.0f ||
            !std::isfinite(values[0]) || values[0] < 0.0f || values[0] > 1.0f ||
            !std::isfinite(values[1]) || values[1] < 0.0f || values[1] > 1.0f) {
            return false;
        }
        m_bypassed = (flags & 1u) != 0;
        m_mix = mix;
        m_sidechainBusId = sidechain;
        setParameter(0, values[0]);
        setParameter(1, values[1]);
        return true;
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
