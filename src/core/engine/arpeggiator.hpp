#pragma once
#include <vector>
#include <algorithm>
#include <random>
#include "../engine_types.hpp"

namespace Aura::Core::Engine {

struct MidiEvent { uint8_t status, data1, data2; };

/**
 * @class Arpeggiator
 * @brief MIDI Rhythmic Pattern Generator.
 * Implements a time-synced pattern generator supporting Up/Down/UpDown/Random modes.
 */
class Arpeggiator {
public:
    enum class Pattern { Up, Down, UpDown, Random };

    Arpeggiator() : m_rng(42) {} 

    void setParameters(Pattern p, int octaves, int stepTicks, float swing = 0.0f) {
        m_pattern = p;
        m_octaves = std::max(1, octaves);
        m_stepTicks = std::max(1, stepTicks);
        m_swing = swing;
    }

    /**
     * @brief Processes incoming MIDI note event chord lists and generates sequential arpeggiated outputs.
     */
    void process(const std::vector<MidiEvent>& in, std::vector<MidiEvent>& out, const EngineContext& ctx) {
        // 1. Process all incoming MIDI events
        for (const auto& ev : in) {
            uint8_t type = ev.status & 0xF0;
            if (type == 0x90 && ev.data2 > 0) { // Note On
                NoteInfo info = { ev.data1, ev.data2 };
                auto it = std::find_if(m_heldNotes.begin(), m_heldNotes.end(), [&](const auto& n) {
                    return n.note == info.note;
                });
                if (it == m_heldNotes.end()) {
                    m_heldNotes.push_back(info);
                }
            } 
            else if (type == 0x80 || (type == 0x90 && ev.data2 == 0)) { // Note Off
                int note = ev.data1;
                m_heldNotes.erase(std::remove_if(m_heldNotes.begin(), m_heldNotes.end(), [&](const auto& n) {
                    return n.note == note;
                }), m_heldNotes.end());
                
                // Pass through note off
                out.push_back(ev);
            }
            else {
                // Pass through other messages
                out.push_back(ev);
            }
        }

        if (m_heldNotes.empty()) {
            if (m_activeArpPitch != -1) {
                out.push_back({ 0x80, static_cast<uint8_t>(m_activeArpPitch), 0 });
                m_activeArpPitch = -1;
            }
            return;
        }

        // 2. Calculate tick position based on sample rate and tempo
        double beatsPerSecond = ctx.tempo / 60.0;
        double totalBeats = (static_cast<double>(ctx.playhead) / ctx.sampleRate) * beatsPerSecond;
        uint64_t totalTicks = static_cast<uint64_t>(totalBeats * MusicalTime::kTicksPerBeat);

        uint32_t step = static_cast<uint32_t>(totalTicks / m_stepTicks);

        if (step != m_lastStepTriggered) {
            // Turn off previous note
            if (m_activeArpPitch != -1) {
                out.push_back({ 0x80, static_cast<uint8_t>(m_activeArpPitch), 0 });
                m_activeArpPitch = -1;
            }

            // Trigger new note
            if (!m_heldNotes.empty()) {
                std::sort(m_heldNotes.begin(), m_heldNotes.end(), [](const auto& a, const auto& b) {
                    return a.note < b.note;
                });

                size_t numNotes = m_heldNotes.size();
                size_t idx = 0;

                switch (m_pattern) {
                    case Pattern::Up:
                        idx = step % numNotes;
                        break;
                    case Pattern::Down:
                        idx = (numNotes - 1) - (step % numNotes);
                        break;
                    case Pattern::UpDown:
                        if (numNotes > 1) {
                            size_t cycle = step % (numNotes * 2 - 2);
                            idx = (cycle < numNotes) ? cycle : (numNotes * 2 - 2) - cycle;
                        } else {
                            idx = 0;
                        }
                        break;
                    case Pattern::Random:
                        idx = std::uniform_int_distribution<size_t>(0, numNotes - 1)(m_rng);
                        break;
                }

                int baseNote = m_heldNotes[idx].note;
                int octaveOffset = (step / numNotes) % m_octaves;
                int finalPitch = baseNote + octaveOffset * 12;
                finalPitch = std::clamp(finalPitch, 0, 127);

                out.push_back({ 0x90, static_cast<uint8_t>(finalPitch), m_heldNotes[idx].velocity });
                m_activeArpPitch = finalPitch;
            }

            m_lastStepTriggered = step;
        }
    }

private:
    struct NoteInfo { int note; uint8_t velocity; };
    std::vector<NoteInfo> m_heldNotes;
    uint32_t m_lastStepTriggered = 0xFFFFFFFF;
    int m_activeArpPitch = -1;
    
    Pattern m_pattern = Pattern::Up;
    int m_octaves = 1;
    int m_stepTicks = 240; 
    float m_swing = 0.0f;
    
    std::mt19937 m_rng;
};

} // namespace Aura::Core::Engine
