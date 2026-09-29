#pragma once

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <vector>

#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** Host, automation, and state adapter for the Rust FET compressor. */
class FETCompressor final : public IProcessor {
public:
    explicit FETCompressor(double sampleRate = 44'100.0)
        : m_state(hirari_fet_compressor_create(sampleRate)) {}
    ~FETCompressor() override { hirari_fet_compressor_destroy(m_state); }

    FETCompressor(const FETCompressor&) = delete;
    FETCompressor& operator=(const FETCompressor&) = delete;

    void prepareToPlay(double sampleRate, uint32_t /*blockSize*/) noexcept override {
        hirari_fet_compressor_prepare(m_state, sampleRate);
    }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (isBypassed() || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        hirari_fet_compressor_process(m_state, buffer.getArrayOfWritePointers(),
                                      buffer.getNumChannels(), buffer.getNumSamples());
    }
    void reset() noexcept override { hirari_fet_compressor_reset(m_state); }
    uint32_t getTailSamples() const noexcept override { return hirari_fet_compressor_tail(m_state); }

    void setThreshold(float db) { setControl(0, db); }
    void setRatio(int ratio) { setControl(1, static_cast<float>(ratio)); }
    void setAttack(float ms) { setControl(2, ms); }
    void setRelease(float ms) { setControl(3, ms); }
    void setParameters(float input, float output, float threshold,
                       float attackMs, float releaseMs, int ratio) {
        hirari_fet_compressor_set_parameters(
            m_state, input, output, threshold, attackMs, releaseMs, ratio);
    }

    std::string getName() const override { return "FET Compressor"; }
    uint32_t getNumParameters() const noexcept override { return 6; }
    void setParameter(uint32_t id, float value) noexcept override {
        hirari_fet_compressor_set_parameter(m_state, id, value);
    }
    float getParameter(uint32_t id) const noexcept override {
        return hirari_fet_compressor_get_parameter(m_state, id);
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= getNumParameters()) return false;
        out = {0.0f, 1.0f, id == 3};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        static constexpr const char* names[] = {
            "Input", "Output", "Threshold", "Ratio", "Attack", "Release"
        };
        std::snprintf(outName, maxSize, "%s", id < 6 ? names[id] : "");
    }

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(40, 0);
        const uint32_t magic = 0x41555241u;
        const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        float values[6]{};
        for (uint32_t id = 0; id < 6; ++id) values[id] = getParameter(id);
        std::memcpy(state.data(), &magic, 4);
        std::memcpy(state.data() + 4, &version, 2);
        std::memcpy(state.data() + 6, &flags, 2);
        std::memcpy(state.data() + 8, &m_mix, 4);
        std::memcpy(state.data() + 12, &m_sidechainBusId, 4);
        std::memcpy(state.data() + 16, values, sizeof(values));
        return state;
    }

    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 40) return false;
        uint32_t magic = 0, sidechain = 0;
        uint16_t version = 0, flags = 0;
        float mix = 0.0f, values[6]{};
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
        for (uint32_t id = 0; id < 6; ++id) setParameter(id, values[id]);
        return true;
    }
    bool restoreStateChecked(const std::vector<uint8_t>& state) override { return setState(state); }

private:
    void setControl(uint32_t control, float value) {
        hirari_fet_compressor_set_control(m_state, control, value);
    }

    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
