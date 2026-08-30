#pragma once

#include <vector>
#include <string>

namespace Aura::Core {

[[nodiscard]] __attribute__((always_inline)) inline float dbToLinear(float db) noexcept {
    return std::pow(10.0f, db * 0.05f);
}

[[nodiscard]] __attribute__((always_inline)) inline float linearToDb(float lin) noexcept {
    return 20.0f * std::log10(std::max(1e-9f, lin));
}

/**
 * @brief SampleBuffer: Zero-copy reference to audio data.
 * Used for massive library handling without memory overhead.
 */
struct SampleBuffer {
    const float* data = nullptr;
    size_t length = 0;
};

/**
 * @struct MIDINote
 * @brief Zero-allocation MIDI note representation.
 */
struct MIDINote {
    uint8_t pitch;
    uint8_t velocity;
    double startBeat;
    double lengthBeats;
};

/**
 * @brief SamplerZone: Mapping information for MIDI-to-Sample routing.
 */
struct SamplerZone {
    uint8_t rootKey = 60;
    uint8_t lowKey = 0;
    uint8_t highKey = 127;
    // Velocity layers are part of the zone contract.  Keeping them here
    // prevents the realtime sampler from silently selecting the first key
    // match and makes multi-layer instruments behave deterministically.
    uint8_t lowVelocity = 1;
    uint8_t highVelocity = 127;
    SampleBuffer buffer;
};

namespace Bridge {
    enum class CommandType : uint8_t {
        Volume = 0, Pan = 1, Solo = 2, Mute = 3, PluginParam = 4,
        SequencerStep = 5, LiveLoopTrigger = 6, LiveLoopStop = 7,
        DrummerPerform = 8, AddNode = 10, ConnectNodes = 11,
        Pan3D = 12, GenerateNeural = 13, MacroValue = 14,
        ExecuteAdvice = 15, FlexMarkerMove = 16, LibraryLatentSeek = 17,
        DrummerElement = 18
    };
}

} // namespace Aura::Core
