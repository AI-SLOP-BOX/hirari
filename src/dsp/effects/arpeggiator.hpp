#pragma once
#include <vector>
#include <array>
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstring>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class Arpeggiator
 * @brief Professional MIDI Arpeggiator with Note-Lifespan management.
 * HONEST FIX: Prevents stuck notes (ghost notes) by tracking and sending 
 * Note-Off messages before each new trigger. Supports 1/16th BPM Sync.
 */
class Arpeggiator : public IProcessor {
public:
    enum class Mode { Up, Down, Range, Random };

    Arpeggiator(double sr = 44100.0) : m_sampleRate(sr) { reset(); }

    std::string getName() const override { return "Arpeggiator"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (id == 0 && std::isfinite(value)) m_mode = static_cast<Mode>(std::clamp(static_cast<int>(std::lround(value)), 0, 3));
    }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? static_cast<float>(m_mode) : 0.0f; }
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
        if (std::isfinite(sr) && sr > 1000.0) m_sampleRate = sr;
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        if (m_bypassed) return;
        (void)buffer;
        const double bpm = std::clamp(std::isfinite(context.bpm) ? context.bpm : 120.0, 20.0, 300.0);
        const double sampleRate = std::isfinite(context.sampleRate) && context.sampleRate >= 1000.0
            ? context.sampleRate : (std::isfinite(m_sampleRate) && m_sampleRate >= 1000.0 ? m_sampleRate : 44100.0);
        const uint64_t stepSamples = std::max<uint64_t>(1, static_cast<uint64_t>(sampleRate * 60.0 / bpm / 4.0));
        m_outputBuffer.clear();
        Core::MidiBuffer& output = m_outputBuffer;
        for (const auto& event : midi) {
            if (event.size < 2 || event.data[0] < 0x80) {
                output.addEvent(event.sampleOffset, event.data, event.size, event.articulationId);
                continue;
            }
            const uint8_t status = event.data[0] & 0xF0;
            const uint8_t channel = static_cast<uint8_t>((event.data[0] & 0x0F) + 1);
            const uint8_t note = event.data[1];
            if (status == 0x90 && event.size >= 3 && event.data[2] != 0) {
                m_channel = channel;
                if (std::find(m_heldNotes.begin(), m_heldNotes.begin() + m_heldCount, note) == m_heldNotes.begin() + m_heldCount && m_heldCount < m_heldNotes.size()) {
                    m_heldNotes[m_heldCount++] = note;
                }
            } else if (status == 0x80 || (status == 0x90 && event.size >= 3 && event.data[2] == 0)) {
                auto it = std::find(m_heldNotes.begin(), m_heldNotes.begin() + m_heldCount, note);
                if (it != m_heldNotes.begin() + m_heldCount) {
                    *it = m_heldNotes[--m_heldCount];
                }
                // Releasing a non-active held key must not cut the currently
                // sounding arp voice. Only stop immediately when the active
                // voice itself was released or the hold set becomes empty.
                if (m_heldCount == 0 || m_activeNote == note) killActiveNote(output, event.sampleOffset);
            } else {
                output.addEvent(event.sampleOffset, event.data, event.size, event.articulationId);
            }
        }
        const uint64_t blockStart = context.blockStart;
        const uint64_t blockEnd = blockStart + buffer.getNumSamples();
        if (m_heldCount > 0 && stepSamples > 0) {
            const uint64_t first = ((blockStart + stepSamples - 1) / stepSamples) * stepSamples;
            for (uint64_t absolute = first; absolute < blockEnd; absolute += stepSamples) {
                killActiveNote(output, absolute - blockStart);
                const size_t index = selectIndex(m_stepCounter++, m_heldCount);
                m_activeNote = m_heldNotes[index];
                output.addNoteOn(m_channel, m_activeNote, 100, absolute - blockStart);
            }
        }
        midi.clear();
        for (const auto& event : output) midi.addEvent(event.sampleOffset, event.data, event.size, event.articulationId);
        midi.sort();
    }


    void reset() noexcept override {
        m_heldCount = 0;
        m_activeNote = 0xFF;
        m_channel = 1;
        m_stepCounter = 0;
        m_outputBuffer.clear();
    }

    size_t selectIndex(uint32_t step, size_t count) const noexcept {
        if (count == 0) return 0;
        switch (m_mode) {
            case Mode::Down: return count - 1 - (step % count);
            case Mode::Range: return (step / count) % 2 == 0 ? step % count : count - 1 - (step % count);
            case Mode::Random: return (static_cast<uint32_t>(step * 1664525u + 1013904223u) >> 16) % count;
            case Mode::Up: default: return step % count;
        }
    }

private:
    void killActiveNote(Core::MidiBuffer& midi, uint32_t offset) {
        if (m_activeNote != 0xFF) {
            uint8_t noteOff[3] = {0x80, m_activeNote, 0};
            midi.addEvent(offset, noteOff, 3);
            m_activeNote = 0xFF;
        }
    }

    double m_sampleRate;
    std::array<uint8_t, 128> m_heldNotes{};
    size_t m_heldCount = 0;
    uint8_t m_activeNote = 0xFF; // Sentinal for 'None'
    uint8_t m_channel = 1;
    uint32_t m_stepCounter = 0;
    Mode m_mode = Mode::Up;
    Core::MidiBuffer m_outputBuffer;
};

} // namespace Aura::DSP::Effects
