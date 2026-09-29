#pragma once
#include <vector>
#include <cmath>
#include <algorithm>
#include <cstring>
#include <cstdio>
#include "../iprocessor.hpp"

namespace Hirari::DSP::Effects {

/**
 * @class AutoFilter
 * @brief Professional Dynamic Resonant Filter (Auto-Wah).
 */
class AutoFilter : public IProcessor {
public:
    AutoFilter() : m_rustEngine(hirari_auto_filter_create()) {}
    ~AutoFilter() override { hirari_auto_filter_destroy(m_rustEngine); }
    AutoFilter(const AutoFilter&) = delete;
    AutoFilter& operator=(const AutoFilter&) = delete;

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        m_sampleRate = (std::isfinite(sr) && sr >= 8000.0 && sr <= 384000.0) ? sr : 44100.0;
        hirari_auto_filter_set_sample_rate(m_rustEngine, m_sampleRate);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        float* channelData[2] = {buffer.getWritePointer(0), nullptr};
        if (channels > 1) channelData[1] = buffer.getWritePointer(1);
        if (!channelData[0] || (channels > 1 && !channelData[1])) return;
        hirari_auto_filter_process(m_rustEngine, channelData, channels,
                                   buffer.getNumSamples(), m_mix);
    }

    void reset() noexcept override { hirari_auto_filter_reset(m_rustEngine); }

    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        if (id < 3) hirari_auto_filter_set_parameter(m_rustEngine, id, value);
        else if (id == 3) m_mix = std::clamp(value, 0.0f, 1.0f);
    }

    std::string getName() const override { return "Auto Filter"; }
    uint32_t getNumParameters() const noexcept override { return 4; }
    float getParameter(uint32_t id) const noexcept override {
        if (id < 3) return hirari_auto_filter_get_parameter(m_rustEngine, id);
        if (id == 3) return m_mix;
        return 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 4) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        const char* names[] = {"Cutoff", "Resonance", "Sensitivity", "Mix"};
        std::snprintf(outName, maxSize, "%s", id < 4 ? names[id] : "");
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(32, 0);
        const uint32_t magic = 0x41555241u; const uint16_t version = 1;
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data()+4, &version, 2);
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        std::memcpy(state.data()+6, &flags, 2); std::memcpy(state.data()+8, &m_sidechainBusId, 4);
        const float values[] = {getParameter(0), getParameter(1), getParameter(2), m_mix};
        std::memcpy(state.data()+16, values, sizeof(values));
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 32) return false;
        uint32_t magic = 0; uint16_t version = 0, flags = 0; uint32_t sidechain = 0;
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data()+4, 2);
        std::memcpy(&flags, state.data()+6, 2); std::memcpy(&sidechain, state.data()+8, 4);
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0) return false;
        float values[4]{}; std::memcpy(values, state.data()+16, sizeof(values));
        for (float value : values) if (!std::isfinite(value) || value < 0.0f || value > 1.0f) return false;
        for (uint32_t i = 0; i < 4; ++i) setParameter(i, values[i]);
        m_bypassed = (flags & 1u) != 0; m_sidechainBusId = sidechain;
        return true;
    }

private:
    double m_sampleRate = 44100.0;
    void* m_rustEngine;
};

} // namespace Hirari::DSP::Effects
