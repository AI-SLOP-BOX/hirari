#pragma once

#include <vector>
#include <algorithm>
#include <cmath>
#include <cstdint>
#include "../../core/midi_buffer.hpp"

namespace Aura::DSP::Effects {

/**
 * @class Arpeggiator
 * @brief Professional Logic-style MIDI Effect Processor.
 * HONEST FIX: Replaced 'Drifting Phase' with 'Global Grid Sync'.
 * Optimized: Sorts held notes only when changed, not every sample.
 */
class Arpeggiator {
public:
    enum class Mode { Up, Down, UpDown, Random, AsPlayed };

    Arpeggiator(double sr = 44100.0) : m_sampleRate(sr) {
        // MIDI has at most 128 distinct pitches. Reserving the full bounded
        // domain prevents a note burst from allocating on the audio thread.
        m_heldNotes.reserve(128);
        m_sortedNotes.reserve(128);
    }

    void setMode(Mode m) { m_mode = m; }

    void process(const Core::MidiBuffer& input, Core::MidiBuffer& output, uint64_t playhead, uint32_t numSamples, float bpm) {
        bool notesChanged = false;

        // 1. Maintain note list (Order matters for AsPlayed)
        const auto* events = input.getEvents();
        for (size_t eventIndex = 0; eventIndex < input.size(); ++eventIndex) {
            const auto& ev = events[eventIndex];
            uint8_t type = ev.data[0] & 0xF0;
            uint8_t note = ev.data[1];
            if (type == 0x90 && ev.data[2] > 0) {
                if (std::find(m_heldNotes.begin(), m_heldNotes.end(), note) == m_heldNotes.end()) {
                    m_heldNotes.push_back(note);
                    notesChanged = true;
                }
            } else if (type == 0x80 || (type == 0x90 && ev.data[2] == 0)) {
                if (m_currentNote == (int)note && m_isNoteActive) {
                    stopCurrentNote(output, ev.sampleOffset);
                }
                auto it = std::remove(m_heldNotes.begin(), m_heldNotes.end(), note);
                if (it != m_heldNotes.end()) {
                    m_heldNotes.erase(it, m_heldNotes.end());
                    notesChanged = true;
                }
            }
        }

        if (notesChanged) {
            m_sortedNotes = m_heldNotes;
            std::sort(m_sortedNotes.begin(), m_sortedNotes.end());
        }

        if (m_heldNotes.empty()) return;

        if (!std::isfinite(bpm) || bpm <= 0.0 ||
            !std::isfinite(m_sampleRate) || m_sampleRate <= 0.0) {
            if (m_isNoteActive) stopCurrentNote(output, 0);
            return;
        }
        const double samplesPerStep = (60.0 / bpm) * m_sampleRate * 0.25;
        if (!std::isfinite(samplesPerStep) || samplesPerStep < 1.0) return;
        double gateWidth = samplesPerStep * 0.8;

        // Optimized Block Processing
        for (uint32_t s = 0; s < numSamples; ++s) {
            uint64_t currentS = playhead + s;
            const uint64_t currentStep = static_cast<uint64_t>(currentS / samplesPerStep);
            double phase = std::fmod(static_cast<double>(currentS), samplesPerStep);

            if (currentStep != m_lastStep) {
                if (m_isNoteActive) stopCurrentNote(output, s);
                
                size_t n = m_heldNotes.size();
                const std::vector<int>& notes = (m_mode == Mode::AsPlayed || m_mode == Mode::Random) ? m_heldNotes : m_sortedNotes;

                int nextNote = -1;
                switch (m_mode) {
                    case Mode::Up: nextNote = notes[currentStep % n]; break;
                    case Mode::Down: nextNote = notes[n - 1 - (currentStep % n)]; break;
                    case Mode::UpDown: {
                        if (n < 2) nextNote = notes[0];
                        else {
                            size_t trip = currentStep % (n * 2 - 2);
                            nextNote = (trip < n) ? notes[trip] : notes[n - 2 - (trip - n)];
                        }
                    } break;
                    case Mode::AsPlayed: nextNote = notes[currentStep % n]; break;
                    case Mode::Random: 
                        nextNote = notes[m_randomSeed % n]; 
                        m_randomSeed = m_randomSeed * 1103515245 + 12345; 
                        break;
                }

                if (nextNote != -1) {
                    uint8_t noteOn[3] = { 0x90, static_cast<uint8_t>(nextNote), 100 };
                    output.addEvent(s, noteOn, 3);
                    m_currentNote = nextNote;
                    m_isNoteActive = true;
                }
                m_lastStep = currentStep;
            } else if (m_isNoteActive && phase >= gateWidth) {
                stopCurrentNote(output, s);
            }
        }
    }

private:
    void stopCurrentNote(Core::MidiBuffer& out, uint32_t sample) {
        if (m_currentNote != -1 && m_isNoteActive) {
            uint8_t noteOff[3] = { 0x80, static_cast<uint8_t>(m_currentNote), 0 };
            out.addEvent(sample, noteOff, 3);
            m_isNoteActive = false;
        }
    }

    double m_sampleRate;
    Mode m_mode = Mode::UpDown;
    std::vector<int> m_heldNotes;
    std::vector<int> m_sortedNotes;
    uint64_t m_lastStep = UINT64_MAX;
    int m_currentNote = -1;
    bool m_isNoteActive = false;
    uint32_t m_randomSeed = 1;
};

} // namespace Aura::DSP::Effects
