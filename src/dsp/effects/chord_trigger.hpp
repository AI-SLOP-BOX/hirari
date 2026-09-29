#pragma once

#include <vector>
#include <algorithm>
#include <cmath>
#include <cstring>
#include <cstdio>
#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

class ChordTrigger : public IProcessor {
public:
    ChordTrigger() : m_state(hirari_chord_trigger_create()) {}
    ~ChordTrigger() override { hirari_chord_trigger_destroy(m_state); }
    ChordTrigger(const ChordTrigger&) = delete;
    ChordTrigger& operator=(const ChordTrigger&) = delete;

    std::string getName() const override { return "Chord Trigger"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (id == 0 && std::isfinite(value)) setStrumMs(std::clamp(value, 0.0f, 1.0f) * 200.0f);
    }
    float getParameter(uint32_t id) const noexcept override {
        return id == 0 ? std::clamp(hirari_chord_trigger_get_strum_ms(m_state) / 200.0f,
                                     0.0f, 1.0f) : 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id != 0) return false; out = {0.0f, 1.0f, false}; return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (outName && maxSize > 0) std::snprintf(outName, maxSize, "%s", id == 0 ? "Strum Time" : "");
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        hirari_chord_trigger_prepare(m_state, sr);
    }

    void reset() noexcept override { hirari_chord_trigger_reset(m_state); }

    /**
     * @brief PROCESS: Injects chord notes with realistic "Strumming" and Velocity Scaling with performance sovereignty.
     * INDUSTRIAL: Delegating MIDI event transformation and strumming to the Rust 'MidiFxOrchestrator'.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)buffer; (void)context;
        if (m_bypassed) return;
        (void)hirari_chord_trigger_process(m_state, midi.rustStateHandle());
    }

    void setStrumMs(float ms) {
        hirari_chord_trigger_set_strum_ms(m_state, ms);
    }

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(24, 0);
        if (!hirari_chord_trigger_save_state(
                m_state, m_bypassed, m_mix, m_sidechainBusId,
                state.data(), state.size())) return {};
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        bool bypassed = false;
        float mix = 0.0f;
        uint32_t sidechainBusId = 0;
        if (!hirari_chord_trigger_restore_state(
                m_state, state.data(), state.size(), &bypassed, &mix, &sidechainBusId))
            return false;
        m_bypassed = bypassed;
        m_mix = mix;
        m_sidechainBusId = sidechainBusId;
        return true;
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
