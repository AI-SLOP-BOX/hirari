#pragma once

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <vector>

#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** Host and persistence adapter for the Rust tape saturation engine. */
class VirtuosoTapeSaturator final : public IProcessor {
public:
    explicit VirtuosoTapeSaturator(double sampleRate = 44'100.0)
        : m_state(hirari_virtuoso_tape_create(sampleRate)) {}
    ~VirtuosoTapeSaturator() override { hirari_virtuoso_tape_destroy(m_state); }

    VirtuosoTapeSaturator(const VirtuosoTapeSaturator&) = delete;
    VirtuosoTapeSaturator& operator=(const VirtuosoTapeSaturator&) = delete;

    void prepareToPlay(double sampleRate, uint32_t /*blockSize*/) noexcept override {
        hirari_virtuoso_tape_prepare(m_state, sampleRate);
    }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (isBypassed() || buffer.isEmpty()) return;
        float* left = buffer.getWritePointer(0);
        if (!left) return;
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        setControl(3, m_mix);
        hirari_virtuoso_tape_process(m_state, left, right, buffer.getNumSamples());
    }
    void reset() noexcept override { hirari_virtuoso_tape_reset(m_state); }
    uint32_t getLatencySamples() const noexcept override { return 50; }
    uint32_t getTailSamples() const noexcept override { return 1024; }
    std::string getName() const override { return "VirtuosoTapeSaturator"; }

    void setDrive(float db) { setControl(0, db); }
    void setHiss(float level) { setControl(1, level); }
    void setWow(float depth) { setControl(2, depth); }
    uint32_t getNumParameters() const noexcept override { return 4; }
    void setParameter(uint32_t id, float value) noexcept override {
        hirari_virtuoso_tape_set_parameter(m_state, id, value);
        if (id == 3 && std::isfinite(value)) setMix(std::clamp(value, 0.0f, 1.0f));
    }
    float getParameter(uint32_t id) const noexcept override {
        if (id == 3) return m_mix;
        return hirari_virtuoso_tape_get_parameter(m_state, id);
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 4) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        static constexpr const char* names[] = {"Drive", "Hiss", "Wow", "Mix"};
        std::snprintf(outName, maxSize, "%s", id < 4 ? names[id] : "");
    }

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(32, 0);
        const uint32_t magic = 0x41555241u;
        const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        float values[4]{};
        for (uint32_t id = 0; id < 4; ++id) values[id] = getParameter(id);
        std::memcpy(state.data(), &magic, 4);
        std::memcpy(state.data() + 4, &version, 2);
        std::memcpy(state.data() + 6, &flags, 2);
        std::memcpy(state.data() + 8, &m_mix, 4);
        std::memcpy(state.data() + 12, &m_sidechainBusId, 4);
        std::memcpy(state.data() + 16, values, sizeof(values));
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 32) return false;
        uint32_t magic = 0, sidechain = 0;
        uint16_t version = 0, flags = 0;
        float mix = 0.0f, values[4]{};
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
        for (uint32_t id = 0; id < 4; ++id) setParameter(id, values[id]);
        return true;
    }
    bool restoreStateChecked(const std::vector<uint8_t>& state) override { return setState(state); }

private:
    void setControl(uint32_t control, float value) {
        hirari_virtuoso_tape_set_control(m_state, control, value);
    }

    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
