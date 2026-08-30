#pragma once
#include <vector>
#include <array>
#include <algorithm>
#include <cmath>
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

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        if (std::isfinite(sr) && sr > 1000.0) m_sampleRate = sr;
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)buffer;
        const double bpm = std::clamp(std::isfinite(context.bpm) ? context.bpm : 120.0, 20.0, 300.0);
        const uint64_t stepSamples = std::max<uint64_t>(1, static_cast<uint64_t>(context.sampleRate * 60.0 / bpm / 4.0));
        Core::MidiBuffer output;
        for (const auto& event : midi) {
            if (event.size < 2 || event.data[0] < 0x80) {
                output.addEvent(event.sampleOffset, event.data, event.size, event.articulationId);
                continue;
            }
            const uint8_t status = event.data[0] & 0xF0;
            const uint8_t channel = static_cast<uint8_t>((event.data[0] & 0x0F) + 1);
            const uint8_t note = event.data[1];
            if (status == 0x90 && event.size >= 3 && event.data[2] != 0) {
                if (std::find(m_heldNotes.begin(), m_heldNotes.begin() + m_heldCount, note) == m_heldNotes.begin() + m_heldCount && m_heldCount < m_heldNotes.size()) {
                    m_heldNotes[m_heldCount++] = note;
                }
            } else if (status == 0x80 || (status == 0x90 && event.size >= 3 && event.data[2] == 0)) {
                auto it = std::find(m_heldNotes.begin(), m_heldNotes.begin() + m_heldCount, note);
                if (it != m_heldNotes.begin() + m_heldCount) {
                    *it = m_heldNotes[--m_heldCount];
                }
                killActiveNote(output, event.sampleOffset);
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
                output.addNoteOn(1, m_activeNote, 100, absolute - blockStart);
            }
        }
        midi.clear();
        for (const auto& event : output) midi.addEvent(event.sampleOffset, event.data, event.size, event.articulationId);
        midi.sort();
    }


    void reset() noexcept override {
        m_heldCount = 0;
        m_activeNote = 0xFF;
        m_stepCounter = 0;
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
    uint32_t m_stepCounter = 0;
    Mode m_mode = Mode::Up;
};

} // namespace Aura::DSP::Effects
