#pragma once

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/// Host-facing adapter for the Rust stereo vocal pitch corrector.
class VocalPitchCorrector final : public IProcessor {
public:
    VocalPitchCorrector() : m_state(hirari_vocal_tuner_create()) {}
    ~VocalPitchCorrector() override { hirari_vocal_tuner_destroy(m_state); }

    VocalPitchCorrector(const VocalPitchCorrector&) = delete;
    VocalPitchCorrector& operator=(const VocalPitchCorrector&) = delete;

    std::string getName() const override { return "Vocal Pitch Corrector"; }
    void setParameter(uint32_t id, float value) noexcept override {
        hirari_vocal_tuner_set_parameter(m_state, id, value);
    }
    float getParameter(uint32_t id) const noexcept override {
        return hirari_vocal_tuner_get_parameter(m_state, id);
    }
    uint32_t getNumParameters() const noexcept override { return 2; }
    float detectedFrequencyHz() const noexcept {
        return hirari_vocal_tuner_detected_frequency(m_state);
    }

    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id > 1) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }

    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        const char* name = id == 0 ? "Correction Amount" : (id == 1 ? "Retune Speed" : "");
        std::snprintf(outName, maxSize, "%s", name);
    }

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(sizeof(float) * 2u);
        const float amount = getParameter(0);
        const float speed = getParameter(1);
        std::memcpy(state.data(), &amount, sizeof(amount));
        std::memcpy(state.data() + sizeof(amount), &speed, sizeof(speed));
        return state;
    }

    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != sizeof(float) * 2u) return false;
        float amount = 0.0f, speed = 0.0f;
        std::memcpy(&amount, state.data(), sizeof(amount));
        std::memcpy(&speed, state.data() + sizeof(amount), sizeof(speed));
        if (!std::isfinite(amount) || !std::isfinite(speed) ||
            amount < 0.0f || amount > 1.0f || speed < 0.0f || speed > 1.0f) return false;
        setParameter(0, amount);
        setParameter(1, speed);
        return true;
    }

    void prepareToPlay(double sampleRate, uint32_t) noexcept override {
        hirari_vocal_tuner_prepare(m_state, sampleRate);
    }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumSamples() == 0 || buffer.getNumChannels() == 0) return;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        if (left && right) hirari_vocal_tuner_process(m_state, left, right, buffer.getNumSamples());
    }
    void reset() noexcept override { hirari_vocal_tuner_reset(m_state); }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
