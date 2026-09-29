#pragma once

#include <cmath>
#include <cstdio>
#include <cstring>
#include <vector>

#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** C++ host and persistence adapter for the Rust stereo phaser. */
class StereoPhaser final : public IProcessor {
public:
    StereoPhaser() : m_state(hirari_stereo_phaser_create(44'100.0)) {}
    ~StereoPhaser() override { hirari_stereo_phaser_destroy(m_state); }

    StereoPhaser(const StereoPhaser&) = delete;
    StereoPhaser& operator=(const StereoPhaser&) = delete;

    std::string getName() const override { return "Stereo Phaser"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }
    uint32_t getNumParameters() const noexcept override { return 3; }
    uint32_t getTailSamples() const noexcept override { return hirari_stereo_phaser_tail(m_state); }

    void setParameter(uint32_t id, float value) noexcept override {
        if (id == 2) setMix(value);
        else hirari_stereo_phaser_set_parameter(m_state, id, value);
    }
    float getParameter(uint32_t id) const noexcept override {
        return id == 2 ? m_mix : hirari_stereo_phaser_get_parameter(m_state, id);
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= getNumParameters()) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        static constexpr const char* names[] = {"Rate", "Feedback", "Mix"};
        std::snprintf(outName, maxSize, "%s", id < 3 ? names[id] : "");
    }

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(28, 0);
        const uint32_t magic = 0x41555241u;
        const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        const float values[3] = {getParameter(0), getParameter(1), getParameter(2)};
        std::memcpy(state.data(), &magic, 4);
        std::memcpy(state.data() + 4, &version, 2);
        std::memcpy(state.data() + 6, &flags, 2);
        std::memcpy(state.data() + 8, &m_mix, 4);
        std::memcpy(state.data() + 12, &m_sidechainBusId, 4);
        std::memcpy(state.data() + 16, values, sizeof(values));
        return state;
    }

    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 28) return false;
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

    void prepareToPlay(double sampleRate, uint32_t /*blockSize*/) noexcept override {
        hirari_stereo_phaser_prepare(m_state, sampleRate);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        hirari_stereo_phaser_process(
            m_state, buffer.getWritePointer(0),
            buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr,
            buffer.getNumSamples(), m_mix);
    }

    void reset() noexcept override { hirari_stereo_phaser_reset(m_state); }
    void setMix(float mix) noexcept { IProcessor::setMix(mix); }
    void setRate(float rate) { hirari_stereo_phaser_set_rate(m_state, rate); }
    void setFeedback(float feedback) { hirari_stereo_phaser_set_feedback(m_state, feedback); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
