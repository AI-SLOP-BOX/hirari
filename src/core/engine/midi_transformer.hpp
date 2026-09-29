#pragma once
#include <vector>
#include <cstdint>
#include "midi_quantizer.hpp"
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

/**
 * @class MidiTransformer
 * @brief Industrial MIDI Logical Editing Engine.
 * HONEST FIX: Implemented tick-based transformations and scale quantization.
 */
class MidiTransformer {
public:
    struct Filter {
        int minPitch = 0, maxPitch = 127;
        int minVel = 0, maxVel = 127;
        uint64_t minLen = 0, maxLen = 0xFFFFFFFF;
    };

    /**
     * @brief Performs batch transformation with industrial precision and musical sovereignty.
     * INDUSTRIAL: Delegating event manipulation and humanization to the Rust 'MidiOrchestrator'.
     */
    static void transform(std::vector<MIDINote>& notes, const Filter& f, 
                          int pitch_offset, float vel_scale, int humanize_ticks) {
        if (notes.empty()) return;
        (void)hirari_midi_transform_notes(
            notes.data(), notes.size(), f.minPitch, f.maxPitch,
            f.minVel, f.maxVel, f.minLen, f.maxLen,
            pitch_offset, vel_scale, humanize_ticks);
    }

    /**
     * @brief SCALE QUANTIZE: Forces notes into a musical key with forensic precision and technical sovereignty.
     * INDUSTRIAL: Delegating scale mapping and key quantization to the Rust 'MidiOrchestrator'.
     */
    static void applyScaleQuantize(std::vector<MIDINote>& notes, uint8_t root, const std::vector<int>& scale) {
        static_assert(sizeof(int) == sizeof(int32_t), "scale degrees require 32-bit integers");
        if (notes.empty() || scale.empty()) return;
        (void)hirari_midi_apply_scale_quantize(
            notes.data(), notes.size(), root,
            reinterpret_cast<const int32_t*>(scale.data()), scale.size());
    }
};

} // namespace Hirari::Core::Engine
