#pragma once

#include <cstdint>
#include <vector>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

struct MIDINote {
    uint64_t startTick;
    uint64_t lengthTicks;
    uint8_t note;
    uint8_t velocity;
};

static_assert(sizeof(MIDINote) == 24, "MIDINote must match the Rust FFI layout");

/**
 * @brief Rust-backed MIDI grid alignment with swing and strength controls.
 * Resolution values are ABI-stable and mirror the Rust enum mapping.
 */
class MidiQuantizer {
public:
    enum class Resolution : uint32_t {
        Q1_4, Q1_8, Q1_16, Q1_32,
        Q1_8T, Q1_16T, Q1_8D, Q1_16D
    };

    static void quantize(std::vector<MIDINote>& notes, Resolution resolution,
                         float swing, float strength) {
        if (notes.empty()) return;
        (void)hirari_midi_quantize(notes.data(), notes.size(),
            static_cast<uint32_t>(resolution), swing, strength, false);
    }

    static void quantizeWithLength(std::vector<MIDINote>& notes, Resolution resolution,
                                   float swing, float strength) {
        if (notes.empty()) return;
        (void)hirari_midi_quantize(notes.data(), notes.size(),
            static_cast<uint32_t>(resolution), swing, strength, true);
    }
};

} // namespace Hirari::Core::Engine
