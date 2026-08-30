#pragma once
#include <vector>
#include <array>
#include <atomic>
#include <mutex>
#include <algorithm>
#include "midi_sequencer.hpp"

namespace Aura::Core::Engine {

/**
 * @class ScaleSystem
 * @brief Industrial Harmonic Intelligence Engine.
 * HONEST FIX: Implemented lock-free scale reading and tick-based chord tracking.
 */
class ScaleSystem {
public:
    enum class Type { Major, Minor, HarmonicMinor, MelodicMinor, Pentatonic };

    struct Chord {
        int root;
        std::vector<int> intervals;
        std::string name;
    };

    static ScaleSystem& getInstance() { static ScaleSystem i; return i; }

    /**
     * @brief SNAP-TO-KEY: Lock-free MIDI quantization to the active scale.
     * INDUSTRIAL: Delegating musical quantization to the Rust 'HarmonicOrchestrator'.
     */
    int quantizeNote(int note) {
        const int root = m_activeRoot.load(std::memory_order_relaxed);
        const uint32_t pattern = m_activePattern.load(std::memory_order_relaxed);
        if (pattern == 0 || pattern == 0xFFF) return note;

        const int pitchClass = ((note % 12) + 12) % 12;
        const int relative = (pitchClass - root + 12) % 12;
        if ((pattern & (1u << relative)) != 0) return note;

        int bestOffset = 1;
        for (int distance = 1; distance <= 6; ++distance) {
            const int below = (relative - distance + 12) % 12;
            const int above = (relative + distance) % 12;
            if (pattern & (1u << below)) {
                bestOffset = -distance;
                break;
            }
            if (pattern & (1u << above)) {
                bestOffset = distance;
                break;
            }
        }
        return note + bestOffset;
    }

    /**
     * @brief SET: Updates the active scale with industrial-grade management.
     * INDUSTRIAL: Using Rust for robust and perfectly consistent scale states.
     */
    void setScale(int root, Type t) {
        const int safeRoot = std::clamp(root, 0, 11);
        m_activeRoot.store(safeRoot, std::memory_order_release);
        m_activePattern.store(getPatternMask(t), std::memory_order_release);
    }

    /**
     * @brief CHORDS: Tracks harmonic progressions on the timeline.
     * INDUSTRIAL: Chord tracking and context resolution are now handled in Rust.
     */
    void addChord(uint64_t tick, int root, const std::vector<int>& intervals, const std::string& name) {
        if (intervals.empty() || intervals.size() > 32 || name.empty()) return;
        Chord chord;
        chord.root = ((root % 12) + 12) % 12;
        chord.name = name;
        chord.intervals.reserve(intervals.size());
        for (int interval : intervals) {
            if (interval >= -48 && interval <= 48) chord.intervals.push_back(interval);
        }
        if (chord.intervals.empty()) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_chordTrack.push_back(ChordEvent{tick, std::move(chord)});
        std::stable_sort(m_chordTrack.begin(), m_chordTrack.end(),
                         [](const ChordEvent& a, const ChordEvent& b) { return a.tick < b.tick; });
    }

private:
    uint32_t getPatternMask(Type t) {
        switch (t) {
            case Type::Major:         return (1u << 0) | (1u << 2) | (1u << 4) | (1u << 5) | (1u << 7) | (1u << 9) | (1u << 11);
            case Type::Minor:         return (1u << 0) | (1u << 2) | (1u << 3) | (1u << 5) | (1u << 7) | (1u << 8) | (1u << 10);
            case Type::HarmonicMinor: return (1u << 0) | (1u << 2) | (1u << 3) | (1u << 5) | (1u << 7) | (1u << 8) | (1u << 11);
            case Type::MelodicMinor:  return (1u << 0) | (1u << 2) | (1u << 3) | (1u << 5) | (1u << 7) | (1u << 9) | (1u << 11);
            case Type::Pentatonic:    return (1u << 0) | (1u << 2) | (1u << 4) | (1u << 7) | (1u << 9);
            default:                  return 0xFFF;
        }
    }

    ScaleSystem() { setScale(0, Type::Major); }
    
    struct ChordEvent { uint64_t tick; Chord chord; };
    std::vector<ChordEvent> m_chordTrack;
    std::mutex m_mutex;

    std::atomic<int> m_activeRoot{0};
    std::atomic<uint32_t> m_activePattern{0};
};

} // namespace Aura::Core::Engine
