#pragma once

#include <cmath>
#include <cstdio>
#include <cstring>
#include <vector>
#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** C++ parameter/state adapter for the Rust ChromaGlow transfer engine. */
class ChromaGlow final : public IProcessor {
public:
    enum class Mode : uint32_t { Retro, Modern, Magnetic };

    explicit ChromaGlow(double sampleRate = 44'100.0)
        : m_state(hirari_chromaglow_create(sampleRate)) {}
    ~ChromaGlow() override { hirari_chromaglow_destroy(m_state); }

    ChromaGlow(const ChromaGlow&) = delete;
    ChromaGlow& operator=(const ChromaGlow&) = delete;

    std::string getName() const override { return "ChromaGlow"; }
    uint32_t getNumParameters() const noexcept override { return 3; }
    void setParameter(uint32_t id, float value) noexcept override {
        hirari_chromaglow_set_parameter(m_state, id, value);
    }
    float getParameter(uint32_t id) const noexcept override {
        return hirari_chromaglow_get_parameter(m_state, id);
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 3) return false;
        out = {0.0f, 1.0f, id == 2};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        static constexpr const char* names[] = {"Drive", "Character", "Mode"};
        std::snprintf(outName, maxSize, "%s", id < 3 ? names[id] : "");
    }

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(28, 0);
        const uint32_t magic = 0x41555241u;
        const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        const float mix = getParameter(1);
        const float values[3] = {getParameter(0), mix, getParameter(2)};
        std::memcpy(state.data(), &magic, 4);
        std::memcpy(state.data() + 4, &version, 2);
        std::memcpy(state.data() + 6, &flags, 2);
        std::memcpy(state.data() + 8, &mix, 4);
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
        m_sidechainBusId = sidechain;
        for (uint32_t id = 0; id < 3; ++id) setParameter(id, values[id]);
        return true;
    }

    void setParams(float driveDb, float character, Mode mode) noexcept {
        hirari_chromaglow_set_params(
            m_state, driveDb, character, static_cast<uint32_t>(mode));
    }
    void setSampleRate(double sampleRate) noexcept {
        hirari_chromaglow_set_sample_rate(m_state, sampleRate);
    }
    void prepareToPlay(double sampleRate, uint32_t) noexcept override {
        hirari_chromaglow_prepare(m_state, sampleRate);
    }
    void process(float* left, float* right, uint32_t frames) noexcept {
        if (left && right && frames != 0) {
            hirari_chromaglow_process(m_state, left, right, frames);
        }
    }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        process(left, right, buffer.getNumSamples());
    }
    void reset() noexcept override { hirari_chromaglow_reset(m_state); }
    uint32_t getLatencySamples() const noexcept override { return 0; }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
