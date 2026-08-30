#pragma once

#include <vector>
#include <map>
#include <algorithm>
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class ScaleAssistant
 * @brief Professional MIDI Scale-Aware Snapping (Scale Quantize).
 * HONEST FIX: Transforms any incoming MIDI note to the nearest musically 
 * correct note within a chosen scale (e.g., C Major, D Minor).
 * Essential for modern producers who want to 'Never Miss a Note' during 
 * live performance or fast composition.
 */
class ScaleAssistant : public IProcessor {
public:
    enum class Scale { Chromatic, Major, Minor, Pentatonic };

    ScaleAssistant() : m_root(0), m_scale(Scale::Major) {
        updateActiveNotes();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {}

    /**
     * @brief PROCESS: Snaps MIDI Note-Ons to the active scale.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)buffer; (void)context;
        m_outputBuffer.clear();
        for (const auto& event : midi) {
            if (event.size < 2 || event.data[0] < 0x80) {
                m_outputBuffer.addEvent(event.sampleOffset, event.data, event.size, event.articulationId);
                continue;
            }
            const uint8_t status = event.data[0] & 0xF0;
            if (status == 0x80 || (status == 0x90 && event.size >= 3 && event.data[2] == 0)) {
                const uint8_t mapped = getNearestNote(event.data[1]);
                uint8_t data[3] = {event.data[0], mapped, static_cast<uint8_t>(event.size >= 3 ? event.data[2] : 0)};
                m_outputBuffer.addEvent(event.sampleOffset, data, 3, event.articulationId);
            } else if (status == 0x90) {
                uint8_t data[3] = {event.data[0], getNearestNote(event.data[1]), event.data[2]};
                m_outputBuffer.addEvent(event.sampleOffset, data, 3, event.articulationId);
            } else {
                m_outputBuffer.addEvent(event.sampleOffset, event.data, event.size, event.articulationId);
            }
        }
        midi.clear();
        for (const auto& event : m_outputBuffer) midi.addEvent(event.sampleOffset, event.data, event.size, event.articulationId);
        midi.sort();
    }


    void reset() noexcept override {}

    // Parameters
    void setRoot(int r) { m_root = r % 12; updateActiveNotes(); }
    void setScale(Scale s) { m_scale = s; updateActiveNotes(); }

private:
    uint8_t getNearestNote(uint8_t n) {
        int best = n;
        int minDist = 128;
        for (int octave = std::max(0, static_cast<int>(n / 12) - 1); octave <= std::min(10, static_cast<int>(n / 12) + 1); ++octave) {
            for (int active : m_activeNotes) {
                const int candidate = octave * 12 + active;
                const int dist = std::abs(candidate - static_cast<int>(n));
                if (candidate >= 0 && candidate <= 127 && dist < minDist) { minDist = dist; best = candidate; }
            }
        }
        return static_cast<uint8_t>(best);
    }

    void updateActiveNotes() {
        m_activeNotes.clear();
        std::vector<int> intervals;
        if (m_scale == Scale::Major) intervals = {0, 2, 4, 5, 7, 9, 11};
        else if (m_scale == Scale::Minor) intervals = {0, 2, 3, 5, 7, 8, 10};
        else if (m_scale == Scale::Pentatonic) intervals = {0, 2, 4, 7, 9};

        for (int i : intervals) m_activeNotes.push_back((m_root + i) % 12);
    }

    int m_root;
    Scale m_scale;
    std::vector<int> m_activeNotes;
    Core::MidiBuffer m_outputBuffer;
};

} // namespace Aura::DSP::Effects
