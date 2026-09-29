#pragma once

#include <cstdio>
#include <cmath>
#include <cstring>
#include <vector>

#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** C++ host/state adapter for the Rust stereo chorus DSP engine. */
class StereoChorus final : public IProcessor {
public:
    StereoChorus() : m_state(hirari_stereo_chorus_create(44'100.0)) {}
    ~StereoChorus() override { hirari_stereo_chorus_destroy(m_state); }

    StereoChorus(const StereoChorus&) = delete;
    StereoChorus& operator=(const StereoChorus&) = delete;

    void prepareToPlay(double sampleRate, uint32_t /*blockSize*/) noexcept override {
        hirari_stereo_chorus_prepare(m_state, sampleRate);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        hirari_stereo_chorus_process(
            m_state, buffer.getWritePointer(0),
            buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr,
            buffer.getNumSamples(), m_mix);
    }

    void reset() noexcept override { hirari_stereo_chorus_reset(m_state); }
    uint32_t getTailSamples() const noexcept override { return 80; }
    std::string getName() const override { return "Stereo Chorus"; }
    uint32_t getNumParameters() const noexcept override { return 2; }

    void setParameter(uint32_t id, float value) noexcept override {
        if (id == 1) setMix(value);
        else hirari_stereo_chorus_set_parameter(m_state, id, value);
    }
    float getParameter(uint32_t id) const noexcept override {
        return id == 1 ? m_mix : hirari_stereo_chorus_get_parameter(m_state, id);
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= getNumParameters()) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        static constexpr const char* names[] = {"Rate", "Mix"};
        std::snprintf(outName, maxSize, "%s", id < 2 ? names[id] : "");
    }

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(24, 0);
        const uint32_t magic = 0x41555241u;
        const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        const float values[2] = {getParameter(0), getParameter(1)};
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

    void setRate(float rate) { hirari_stereo_chorus_set_rate(m_state, rate); }
    void setMix(float mix) noexcept { IProcessor::setMix(mix); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
