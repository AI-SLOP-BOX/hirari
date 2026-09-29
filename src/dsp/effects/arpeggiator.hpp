#pragma once
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstring>
#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/**
 * @class Arpeggiator
 * @brief Professional MIDI Arpeggiator with Note-Lifespan management.
 * HONEST FIX: Prevents stuck notes (ghost notes) by tracking and sending 
 * Note-Off messages before each new trigger. Supports 1/16th BPM Sync.
 */
class Arpeggiator : public IProcessor {
public:
    enum class Mode { Up, Down, Range, Random };

    Arpeggiator(double sr = 44100.0) : m_state(hirari_arpeggiator_create(sr)) {
        hirari_arpeggiator_reset(m_state);
    }
    ~Arpeggiator() override { hirari_arpeggiator_destroy(m_state); }
    Arpeggiator(const Arpeggiator&) = delete;
    Arpeggiator& operator=(const Arpeggiator&) = delete;

    std::string getName() const override { return "Arpeggiator"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (id == 0 && std::isfinite(value)) {
            hirari_arpeggiator_set_mode(
                m_state, static_cast<uint32_t>(std::clamp(static_cast<int>(std::lround(value)), 0, 3)));
        }
    }
    float getParameter(uint32_t id) const noexcept override {
        return id == 0 ? static_cast<float>(hirari_arpeggiator_get_mode(m_state)) : 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id != 0) return false; out = {0.0f, 3.0f, true}; return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (outName && maxSize > 0) std::snprintf(outName, maxSize, "%s", id == 0 ? "Pattern" : "");
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(24, 0); const uint32_t magic = 0x41555241u; const uint16_t version = 1; const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data()+4, &version, 2); std::memcpy(state.data()+6, &flags, 2); std::memcpy(state.data()+8, &m_mix, 4); std::memcpy(state.data()+12, &m_sidechainBusId, 4);
        const float pattern = getParameter(0); std::memcpy(state.data()+16, &pattern, 4); return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 24) return false; uint32_t magic=0, sidechain=0; uint16_t version=0, flags=0; float mix=0.0f, pattern=0.0f;
        std::memcpy(&magic,state.data(),4); std::memcpy(&version,state.data()+4,2); std::memcpy(&flags,state.data()+6,2); std::memcpy(&mix,state.data()+8,4); std::memcpy(&sidechain,state.data()+12,4); std::memcpy(&pattern,state.data()+16,4);
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f || !std::isfinite(pattern) || pattern < 0.0f || pattern > 3.0f) return false;
        m_bypassed=(flags&1u)!=0; m_mix=mix; m_sidechainBusId=sidechain; setParameter(0,pattern); return true;
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        hirari_arpeggiator_prepare(m_state, sr);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        if (m_bypassed) return;
        (void)buffer;
        const size_t outputCount = hirari_arpeggiator_process(
            m_state, midi.getEvents(), midi.size(), context.bpm, context.sampleRate,
            context.blockStart, buffer.getNumSamples());
        const auto* outputEvents = static_cast<const Core::MidiEvent*>(
            hirari_arpeggiator_output(m_state));
        midi.clear();
        for (size_t index = 0; outputEvents && index < outputCount; ++index) {
            midi.tryAddEvent(outputEvents[index]);
        }
        midi.sort();
    }


    void reset() noexcept override {
        hirari_arpeggiator_reset(m_state);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
