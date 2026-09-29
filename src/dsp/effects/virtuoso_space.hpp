#pragma once

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdint>
#include <cstring>
#include <string>
#include <vector>
#include "../../core/audio_buffer.hpp"
#include "../../core/rust_ffi.hpp"
#include "../iprocessor.hpp"

namespace Hirari::DSP::Effects {

// IProcessor adapter; the FDN state and sample processing live in Rust.
class VirtuosoSpace final : public IProcessor {
public:
    explicit VirtuosoSpace(double sampleRate = 44'100.0)
        : m_state(hirari_virtuoso_space_create(sampleRate)) {}
    ~VirtuosoSpace() override { hirari_virtuoso_space_destroy(m_state); }
    VirtuosoSpace(const VirtuosoSpace&) = delete;
    VirtuosoSpace& operator=(const VirtuosoSpace&) = delete;

    std::string getName() const override { return "Virtuoso Space"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }
    uint32_t getNumParameters() const noexcept override { return 4; }

    void setParameter(uint32_t id, float value) noexcept override {
        hirari_virtuoso_space_set_parameter(m_state, id, value);
        if (id == 3 && std::isfinite(value)) setMix(std::clamp(value, 0.0f, 1.0f));
    }

    float getParameter(uint32_t id) const noexcept override {
        return hirari_virtuoso_space_get_parameter(m_state, id);
    }

    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 4) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }

    void getParameterName(uint32_t id, char* output, uint32_t capacity) const noexcept override {
        if (!output || capacity == 0) return;
        static constexpr const char* names[] = {"Decay", "Damping", "Size", "Mix"};
        std::snprintf(output, capacity, "%s", id < 4 ? names[id] : "");
    }

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(32, 0);
        hirari_virtuoso_space_write_state(
            m_state, state.data(), state.size(), isBypassed(), getSidechainBus());
        return state;
    }

    bool setState(const std::vector<uint8_t>& state) override {
        if (!hirari_virtuoso_space_set_state(m_state, state.data(), state.size())) return false;
        if (state.size() != 32) return false;
        uint16_t flags = 0;
        uint32_t sidechain = 0;
        std::memcpy(&flags, state.data() + 6, sizeof(flags));
        std::memcpy(&sidechain, state.data() + 12, sizeof(sidechain));
        setMix(getParameter(3));
        setBypassed((flags & 1u) != 0);
        setSidechainBus(sidechain);
        return true;
    }

    void prepareToPlay(double sampleRate, uint32_t) noexcept override {
        hirari_virtuoso_space_prepare(m_state, sampleRate);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const ProcessContext&) noexcept override {
        if (isBypassed() || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        hirari_virtuoso_space_process(m_state, left, right, buffer.getNumSamples());
    }

    void reset() noexcept override { hirari_virtuoso_space_reset(m_state); }
    uint32_t getTailSamples() const noexcept override {
        return hirari_virtuoso_space_tail_samples(m_state);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
