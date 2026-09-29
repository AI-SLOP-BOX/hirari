#pragma once
#include <vector>
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <limits>
#include <string>
#include <utility>
#include "midi_sequencer.hpp"
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

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
        return hirari_scale_quantizer_note(m_quantizer, note);
    }

    /**
     * @brief SET: Updates the active scale with industrial-grade management.
     * INDUSTRIAL: Using Rust for robust and perfectly consistent scale states.
     */
    void setScale(int root, Type t) {
        hirari_scale_quantizer_set(m_quantizer, std::clamp(root, 0, 11),
                                   static_cast<uint32_t>(t));
    }

    /**
     * @brief CHORDS: Tracks harmonic progressions on the timeline.
     * INDUSTRIAL: Chord tracking and context resolution are now handled in Rust.
     */
    void addChord(uint64_t tick, int root, const std::vector<int>& intervals, const std::string& name) {
        static_assert(sizeof(int) == sizeof(int32_t), "chord intervals require 32-bit integers");
        if (intervals.empty() || intervals.size() > 32 || name.empty()) return;
        (void)hirari_scale_quantizer_add_chord(
            m_quantizer, tick, root,
            reinterpret_cast<const int32_t*>(intervals.data()), intervals.size(),
            reinterpret_cast<const uint8_t*>(name.data()), name.size());
    }

    Chord getChordAt(double beat) const {
        if (!std::isfinite(beat) || beat < 0.0) return {};
        const long double tickValue = static_cast<long double>(beat) * 960.0L;
        const uint64_t tick = tickValue >= static_cast<long double>(UINT64_MAX)
            ? UINT64_MAX : static_cast<uint64_t>(tickValue);
        int32_t root = 0;
        int32_t intervals[32]{};
        size_t intervalCount = 0;
        size_t nameLength = 0;
        uint8_t status = hirari_scale_quantizer_chord_at(
            m_quantizer, tick, &root, intervals, 32, &intervalCount,
            nullptr, 0, &nameLength);
        if (status == 0 || intervalCount > 32) return {};
        std::string name(nameLength, '\0');
        status = hirari_scale_quantizer_chord_at(
            m_quantizer, tick, &root, intervals, 32, &intervalCount,
            reinterpret_cast<uint8_t*>(name.data()), name.size(), &nameLength);
        if (status != 1 || intervalCount > 32 || nameLength > name.size()) return {};
        name.resize(nameLength);
        Chord chord;
        chord.root = root;
        chord.intervals.assign(intervals, intervals + intervalCount);
        chord.name = std::move(name);
        return chord;
    }

private:
    ScaleSystem() : m_quantizer(hirari_scale_quantizer_create()) {}
    ~ScaleSystem() { hirari_scale_quantizer_destroy(m_quantizer); }
    ScaleSystem(const ScaleSystem&) = delete;
    ScaleSystem& operator=(const ScaleSystem&) = delete;

    void* m_quantizer = nullptr;
};

} // namespace Hirari::Core::Engine
