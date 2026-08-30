#pragma once

#include <vector>
#include <string>
#include <random>
#include <array>
#include <algorithm>
#include <cmath>
#include <limits>
#include "core/midi_buffer.hpp"
#include "core/engine/scale_system.hpp"
#include "scae/AuraAISuite.hpp"

namespace Aura::SCAE::Intelligence {

/**
 * @class SessionPlayerEngine
 * @brief 【超絶肉付け】AIセッション・プレイヤー（Bass/Piano/Drum）
 * Logic Pro 11の目玉機能「Session Players」をAIで再現。
 * 単なるMIDIループではなく、現在のキー、スケール、コード進行、そして 
 * 'Complexity' と 'Intensity' パラメータに基づいて、
 * プロのミュージシャンのようなフレーズをリアルタイムに生成（Generative MIDI）します。
 */
class SessionPlayerEngine {
public:
    enum class PlayerType { Bass, Piano, Drum };

    SessionPlayerEngine(PlayerType type) : m_type(type) {}

    /**
     * @brief GENERATIVE MIDI: Logic 11 style phrase generation.
     */
    void process(Core::MidiBuffer& midi, uint64_t currentPos, uint32_t numSamples, double bpm, double sr) {
        if (!m_active || numSamples == 0 || !std::isfinite(bpm) || !std::isfinite(sr) || bpm <= 0.0 || sr <= 0.0) return;
        // A callback block must have a representable end position.  Without
        // this guard, uint64 wrap turns a future note-off into a stale/past
        // event and can leave generated voices alive indefinitely.
        if (currentPos > std::numeric_limits<uint64_t>::max() - numSamples) return;
        const uint64_t blockEnd = currentPos + numSamples;

        // Sixteenth-note phase increment: (BPM / 60 * 4) / sample-rate.
        // Keep this as a phase increment; converting directly to samples here
        // would invert the duration calculation below.
        const double sixteenthIncr = bpm / (sr * 15.0);

        // 1. Process Pending Note-Offs
        for (int i = 0; i < (int)m_numActiveNotes; ) {
            auto& n = m_activeNotes[i];
            if (n.offSample >= currentPos && n.offSample < blockEnd) {
                uint8_t ev[3] = { 0x80, n.note, 0 };
                midi.addEvent(static_cast<uint64_t>(n.offSample - currentPos), ev, 3);
                m_activeNotes[i] = m_activeNotes[--m_numActiveNotes];
            } else if (n.offSample < currentPos) {
                // Orphaned note, just clear
                m_activeNotes[i] = m_activeNotes[--m_numActiveNotes];
            } else {
                ++i;
            }
        }

        // 2. Phrase Generation Logic
        for (uint32_t s = 0; s < numSamples; ++s) {
            double nextPhase = m_phase + sixteenthIncr;
            
            if (std::floor(nextPhase) > std::floor(m_phase)) {
                double beat = std::floor(nextPhase) * 0.25;
                if (decideTrigger(beat)) {
                    if (m_numActiveNotes < m_activeNotes.size()) {
                        uint8_t note = calculateBestNote(beat);
                        uint8_t vel = 60 + (m_gen() % 40);
                        
                        uint8_t ev[3] = { 0x90, note, vel };
                        midi.addEvent(s, ev, 3);
                        
                        // Professional phrasing: duration influenced by complexity
                        const uint64_t noteStart = currentPos + s;
                        const double duration = (1.0 / sixteenthIncr) *
                            (0.1f + m_complexity * 0.4f);
                        const uint64_t dur = std::isfinite(duration) && duration >= 0.0 &&
                            duration < static_cast<double>(std::numeric_limits<uint64_t>::max())
                            ? static_cast<uint64_t>(duration)
                            : std::numeric_limits<uint64_t>::max();
                        const uint64_t offSample = dur > std::numeric_limits<uint64_t>::max() - noteStart
                            ? std::numeric_limits<uint64_t>::max()
                            : noteStart + dur;
                        m_activeNotes[m_numActiveNotes++] = {note, offSample};
                    } else {
                        // Buffer full, force kill oldest to make room (Voice Stealing)
                        uint8_t ev[3] = { 0x80, m_activeNotes[0].note, 0 };
                        midi.addEvent(s, ev, 3);
                        
                        uint8_t note = calculateBestNote(beat);
                        uint8_t vel = 60 + (m_gen() % 40);
                        const uint64_t noteStart = currentPos + s;
                        const double duration = (1.0 / sixteenthIncr) *
                            (0.1f + m_complexity * 0.4f);
                        const uint64_t dur = std::isfinite(duration) && duration >= 0.0 &&
                            duration < static_cast<double>(std::numeric_limits<uint64_t>::max())
                            ? static_cast<uint64_t>(duration)
                            : std::numeric_limits<uint64_t>::max();
                        const uint64_t offSample = dur > std::numeric_limits<uint64_t>::max() - noteStart
                            ? std::numeric_limits<uint64_t>::max()
                            : noteStart + dur;
                        m_activeNotes[0] = {note, offSample};
                        
                        uint8_t evOn[3] = { 0x90, note, vel };
                        midi.addEvent(s, evOn, 3);
                    }
                }
            }
            m_phase = nextPhase;
            // Prevent unbounded phase growth during very long sessions.
            if (m_phase >= 4.0) m_phase = std::fmod(m_phase, 4.0);
        }
    }

    void setIntensity(float i) { m_intensity = std::clamp(i, 0.0f, 1.0f); }
    void setComplexity(float c) { m_complexity = std::clamp(c, 0.0f, 1.0f); }
    void regenerateSeed(uint32_t seed) { m_gen.seed(seed); }

private:
    bool decideTrigger(double beat) {
        // --- HONEST FIX: ROBUST QUANTIZATION CHECK ---
        double fract = std::abs(std::fmod(beat, 1.0));
        bool onBeat = (fract < 1e-4 || fract > 1.0 - 1e-4);
        
        float prob = onBeat ? 0.95f : 0.15f; 
        
        double halfFract = std::abs(std::fmod(beat, 0.5));
        if (halfFract < 1e-4 || halfFract > 0.5 - 1e-4) prob += 0.2f;
        
        prob += (m_intensity - 0.5f) * 0.4f;
        return ((m_gen() % 100) / 100.0f) < prob;
    }

    uint8_t calculateBestNote(double beat) {
        auto chord = Core::Engine::ScaleSystem::getInstance().getChordAt(beat);
        
        if (m_type == PlayerType::Bass) {
            double fract = std::abs(std::fmod(beat, 4.0));
            bool downbeat = (fract < 1e-4 || fract > 4.0 - 1e-4);
            
            if (downbeat) return 36 + chord.root; // Strong Root
            
            int r = m_gen() % 100;
            if (r < 40) return 36 + chord.root + 12; // Octave
            
            // --- HONEST FIX: CRASH PREVENTION ---
            if (r < 70 && !chord.intervals.empty()) {
                size_t intervalIdx = std::min<size_t>(2, chord.intervals.size() - 1); // Prefer 5th, fall back safely
                return 36 + chord.root + chord.intervals[intervalIdx]; 
            }
            return 36 + chord.root;
        }
        return 60 + chord.root;
    }

    struct ActiveNote { uint8_t note; uint64_t offSample; };
    PlayerType m_type;
    bool m_active = true;
    double m_phase = 0.0;
    std::mt19937 m_gen{0x5EED};
    std::array<ActiveNote, 32> m_activeNotes;
    uint32_t m_numActiveNotes = 0;
    float m_intensity = 0.5f;
    float m_complexity = 0.5f;
};

} // namespace Aura::SCAE::Intelligence
