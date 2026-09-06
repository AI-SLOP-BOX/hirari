#pragma once

#include <vector>
#include <array>
#include <algorithm>
#include <cmath>
#include <cstring>
#include <cstdio>
#include <atomic>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

class ChordTrigger : public IProcessor {
public:
    ChordTrigger() {
        m_chordIntervals = {0, 4, 7}; // Major Triad
    }

    std::string getName() const override { return "Chord Trigger"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (id == 0 && std::isfinite(value)) setStrumMs(std::clamp(value, 0.0f, 1.0f) * 200.0f);
    }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? std::clamp(m_strumMs.load(std::memory_order_relaxed) / 200.0f, 0.0f, 1.0f) : 0.0f; }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id != 0) return false; out = {0.0f, 1.0f, false}; return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (outName && maxSize > 0) std::snprintf(outName, maxSize, "%s", id == 0 ? "Strum Time" : "");
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = sr;
        updateStrumSamples();
    }

    void reset() noexcept override { m_outputBuffer.clear(); }

    /**
     * @brief PROCESS: Injects chord notes with realistic "Strumming" and Velocity Scaling with performance sovereignty.
     * INDUSTRIAL: Delegating MIDI event transformation and strumming to the Rust 'MidiFxOrchestrator'.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)buffer; (void)context;
        if (m_bypassed) return;
        m_outputBuffer.clear();
        for (const auto& event : midi) {
            if (event.size < 3) { m_outputBuffer.addEvent(event.sampleOffset, event.data, event.size, event.articulationId); continue; }
            const uint8_t status = event.data[0] & 0xF0;
            const uint8_t channel = static_cast<uint8_t>((event.data[0] & 0x0F) + 1);
            const uint8_t note = event.data[1];
            if (status == 0x90 && event.data[2] != 0) {
                for (size_t j = 0; j < m_numIntervals; ++j) {
                    const int pitch = std::clamp(static_cast<int>(note) + m_chordIntervals[j], 0, 127);
                    const uint64_t strumOffset = std::min<uint64_t>(
                        static_cast<uint64_t>(m_cachedStrumSamples.load(std::memory_order_relaxed)) * j,
                        0xFFFFFFFFull);
                    const uint64_t offset = event.sampleOffset + strumOffset;
                    m_outputBuffer.addNoteOn(channel, static_cast<uint8_t>(pitch), event.data[2], offset, event.articulationId);
                }
            } else if (status == 0x80 || (status == 0x90 && event.data[2] == 0)) {
                for (size_t j = 0; j < m_numIntervals; ++j) {
                    const int pitch = std::clamp(static_cast<int>(note) + m_chordIntervals[j], 0, 127);
                    m_outputBuffer.addNoteOff(channel, static_cast<uint8_t>(pitch), event.sampleOffset);
                }
            } else {
                m_outputBuffer.addEvent(event.sampleOffset, event.data, event.size, event.articulationId);
            }
        }
        midi.clear();
        for (const auto& event : m_outputBuffer) midi.addEvent(event.sampleOffset, event.data, event.size, event.articulationId);
        midi.sort();
    }

    void setStrumMs(float ms) {
        m_strumMs.store(std::clamp(std::isfinite(ms) ? ms : 15.0f, 0.0f, 200.0f), std::memory_order_relaxed);
        updateStrumSamples();
    }

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(24, 0); const uint32_t magic = 0x41555241u; const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u); const float strum = m_strumMs.load(std::memory_order_relaxed);
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data()+4, &version, 2); std::memcpy(state.data()+6, &flags, 2);
        std::memcpy(state.data()+8, &m_mix, 4); std::memcpy(state.data()+12, &m_sidechainBusId, 4); std::memcpy(state.data()+16, &strum, 4);
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 24) return false;
        uint32_t magic = 0, sidechain = 0; uint16_t version = 0, flags = 0; float mix = 0.0f, strum = 0.0f;
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data()+4, 2); std::memcpy(&flags, state.data()+6, 2);
        std::memcpy(&mix, state.data()+8, 4); std::memcpy(&sidechain, state.data()+12, 4); std::memcpy(&strum, state.data()+16, 4);
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f || !std::isfinite(strum) || strum < 0.0f || strum > 200.0f) return false;
        m_bypassed = (flags & 1u) != 0; m_mix = mix; m_sidechainBusId = sidechain; setStrumMs(strum); return true;
    }

private:
    void updateStrumSamples() {
        m_cachedStrumSamples.store(static_cast<uint32_t>((m_strumMs.load(std::memory_order_relaxed) / 1000.0) * m_sampleRate), std::memory_order_relaxed);
    }

    std::array<int, 12> m_chordIntervals;
    size_t m_numIntervals = 3;
    std::atomic<float> m_strumMs{15.0f};
    std::atomic<uint32_t> m_cachedStrumSamples{0};
    double m_sampleRate = 44100.0;
    Core::MidiBuffer m_outputBuffer;
};

} // namespace Aura::DSP::Effects
