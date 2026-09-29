#pragma once

#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <memory>
#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::Core::DSP::Effects {

/// C++ host adapter for the Rust real-time pitch correction engine.
class AutoPitchCorrector final : public ::Hirari::DSP::IProcessor {
public:
    explicit AutoPitchCorrector(double sampleRate, size_t fftSize = 1024)
        : m_state(hirari_auto_pitch_create(sampleRate, fftSize)) {}
    ~AutoPitchCorrector() override { hirari_auto_pitch_destroy(m_state); }

    AutoPitchCorrector(const AutoPitchCorrector&) = delete;
    AutoPitchCorrector& operator=(const AutoPitchCorrector&) = delete;

    void prepareToPlay(double sampleRate, uint32_t) noexcept override {
        hirari_auto_pitch_prepare(m_state, sampleRate);
    }

    void process(::Hirari::Core::AudioBuffer& buffer, ::Hirari::Core::MidiBuffer&,
                 const ::Hirari::DSP::ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        if (!left || !right) return;
        hirari_auto_pitch_process(m_state, left, right, buffer.getNumSamples());
    }

    void reset() noexcept override { hirari_auto_pitch_reset(m_state); }
    uint32_t getLatencySamples() const noexcept override {
        return hirari_auto_pitch_latency(m_state);
    }
    uint32_t getTailSamples() const noexcept override { return getLatencySamples(); }
    std::string getName() const override { return "Auto Pitch Corrector"; }
    uint32_t getNumParameters() const noexcept override { return 2; }

    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id == 0) { out = {0.0f, 1.0f, false}; return true; }
        if (id == 1) { out = {0.0f, 4095.0f, true}; return true; }
        return false;
    }

    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        const char* name = id == 0 ? "Correction Response" : (id == 1 ? "Scale Mask" : "");
        std::snprintf(outName, maxSize, "%s", name);
    }

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(32, 0);
        const uint32_t magic = 0x41555241u;
        const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        const float response = getParameter(0);
        const float mask = getParameter(1);
        std::memcpy(state.data(), &magic, sizeof(magic));
        std::memcpy(state.data() + 4, &version, sizeof(version));
        std::memcpy(state.data() + 6, &flags, sizeof(flags));
        std::memcpy(state.data() + 8, &m_mix, sizeof(m_mix));
        std::memcpy(state.data() + 12, &m_sidechainBusId, sizeof(m_sidechainBusId));
        std::memcpy(state.data() + 16, &response, sizeof(response));
        std::memcpy(state.data() + 20, &mask, sizeof(mask));
        return state;
    }

    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 32) return false;
        uint32_t magic = 0, sidechain = 0;
        uint16_t version = 0, flags = 0;
        float mix = 0.0f, response = 0.0f, mask = 0.0f;
        std::memcpy(&magic, state.data(), 4);
        std::memcpy(&version, state.data() + 4, 2);
        std::memcpy(&flags, state.data() + 6, 2);
        std::memcpy(&mix, state.data() + 8, 4);
        std::memcpy(&sidechain, state.data() + 12, 4);
        std::memcpy(&response, state.data() + 16, 4);
        std::memcpy(&mask, state.data() + 20, 4);
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 ||
            !std::isfinite(mix) || mix < 0.0f || mix > 1.0f ||
            !std::isfinite(response) || response < 0.0f || response > 1.0f ||
            !std::isfinite(mask) || mask < 0.0f || mask > 4095.0f) return false;
        m_bypassed = (flags & 1u) != 0;
        m_mix = mix;
        m_sidechainBusId = sidechain;
        setParameter(0, response);
        setParameter(1, mask);
        return true;
    }

    void setParameter(uint32_t id, float value) noexcept override {
        hirari_auto_pitch_set_parameter(m_state, id, value);
    }
    float getParameter(uint32_t id) const noexcept override {
        return hirari_auto_pitch_get_parameter(m_state, id);
    }
    float getCurrentPitch() const noexcept {
        return hirari_auto_pitch_detected_frequency(m_state);
    }
    float getCorrectionAmount() const noexcept {
        return hirari_auto_pitch_correction_amount(m_state);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core::DSP::Effects
