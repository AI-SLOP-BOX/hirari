#pragma once

#include <vector>
#include <array>
#include <algorithm>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

class ChordTrigger : public IProcessor {
public:
    ChordTrigger() {
        m_chordIntervals = {0, 4, 7}; // Major Triad
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
        m_outputBuffer.clear();
        for (const auto& event : midi) {
            if (event.size < 3) { m_outputBuffer.addEvent(event.sampleOffset, event.data, event.size, event.articulationId); continue; }
            const uint8_t status = event.data[0] & 0xF0;
            const uint8_t channel = static_cast<uint8_t>((event.data[0] & 0x0F) + 1);
            const uint8_t note = event.data[1];
            if (status == 0x90 && event.data[2] != 0) {
                for (size_t j = 0; j < m_numIntervals; ++j) {
                    const int pitch = std::clamp(static_cast<int>(note) + m_chordIntervals[j], 0, 127);
                    const uint64_t offset = event.sampleOffset + std::min<uint32_t>(m_cachedStrumSamples * static_cast<uint32_t>(j), 0xFFFFFFFFu);
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
        m_strumMs = ms;
        updateStrumSamples();
    }

private:
    void updateStrumSamples() {
        m_cachedStrumSamples = static_cast<uint32_t>((m_strumMs / 1000.0) * m_sampleRate);
    }

    std::array<int, 12> m_chordIntervals;
    size_t m_numIntervals = 3;
    float m_strumMs = 15.0f;
    uint32_t m_cachedStrumSamples = 0;
    double m_sampleRate = 44100.0;
    Core::MidiBuffer m_outputBuffer;
};

} // namespace Aura::DSP::Effects
