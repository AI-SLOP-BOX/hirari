#pragma once

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstring>
#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/// C++ host adapter for the Rust FFT overlap-add linear phase equalizer.
class LinearPhaseEQ final : public IProcessor {
public:
    static constexpr size_t kFFTSize = 1024;

    LinearPhaseEQ() : m_state(hirari_linear_phase_eq_create()) {}
    ~LinearPhaseEQ() override { hirari_linear_phase_eq_destroy(m_state); }
    LinearPhaseEQ(const LinearPhaseEQ&) = delete;
    LinearPhaseEQ& operator=(const LinearPhaseEQ&) = delete;

    void prepareToPlay(double, uint32_t) noexcept override { reset(); }
    uint32_t getLatencySamples() const noexcept override { return kFFTSize / 2; }
    uint32_t getTailSamples() const noexcept override { return kFFTSize - 1; }
    std::string getName() const override { return "Linear Phase EQ"; }

    void setParameter(uint32_t id, float value) noexcept override {
        if (id >= 3 || !std::isfinite(value)) return;
        float gains[3] = {getParameter(0) * 16.0f,
                          getParameter(1) * 16.0f,
                          getParameter(2) * 16.0f};
        gains[id] = sanitizeGain(value * 16.0f);
        setGain(gains[0], gains[1], gains[2]);
    }
    float getParameter(uint32_t id) const noexcept override {
        return id < 3 ? std::clamp(hirari_linear_phase_eq_get_gain(m_state, id) / 16.0f,
                                    0.0f, 1.0f) : 0.0f;
    }
    uint32_t getNumParameters() const noexcept override { return 3; }

    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 3) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        const char* name = id == 0 ? "Low Gain" : (id == 1 ? "Mid Gain" : (id == 2 ? "High Gain" : ""));
        std::snprintf(outName, maxSize, "%s", name);
    }

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(sizeof(float) * 3u);
        const float gains[3] = {getParameter(0), getParameter(1), getParameter(2)};
        std::memcpy(state.data(), gains, sizeof(gains));
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != sizeof(float) * 3u) return false;
        float gains[3]{};
        std::memcpy(gains, state.data(), sizeof(gains));
        for (float gain : gains)
            if (!std::isfinite(gain) || gain < 0.0f || gain > 1.0f) return false;
        setParameter(0, gains[0]);
        setParameter(1, gains[1]);
        setParameter(2, gains[2]);
        return true;
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumSamples() == 0 || buffer.getNumChannels() == 0) return;
        hirari_linear_phase_eq_process(m_state, buffer.getArrayOfWritePointers(),
                                       buffer.getNumChannels(), buffer.getNumSamples());
    }
    void reset() noexcept override { hirari_linear_phase_eq_reset(m_state); }

    void setGain(float low, float mid, float high) noexcept {
        hirari_linear_phase_eq_set_gains(
            m_state, sanitizeGain(low), sanitizeGain(mid), sanitizeGain(high));
    }

private:
    static float sanitizeGain(float gain) noexcept {
        return std::isfinite(gain) ? std::clamp(gain, 0.0f, 16.0f) : 1.0f;
    }

    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
