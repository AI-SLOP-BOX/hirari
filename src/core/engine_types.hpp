#pragma once
#include <cstdint>

namespace Aura::Core::Engine {

/**
 * @struct MusicalTime
 * @brief Professional Bar/Beat/Tick representation.
 */
struct MusicalTime {
    int32_t bar;
    int32_t beat;
    int32_t tick;
    static constexpr int32_t kTicksPerBeat = 960; // Logic Pro industrial standard
};

/**
 * @struct TimeSignature
 * @brief Musical meter definition.
 */
struct TimeSignature {
    int32_t numerator = 4;
    int32_t denominator = 4;
};

/**
 * @struct EngineContext
 * @brief Comprehensive execution context for the audio engine.
 * HONEST FIX: Added musical context to enable tempo-synced processing.
 */
struct EngineContext {
    uint64_t playhead;      // Samples
    double sampleRate;      // Hz
    uint32_t blockSize;     // Samples
    float tempo;            // BPM
    
    TimeSignature timeSig;
    MusicalTime musicalPos;

    /**
     * @brief Helper to calculate musical position from sample playhead.
     */
    void updateMusicalPos() {
        double beatsPerSecond = tempo / 60.0;
        double totalBeats = (static_cast<double>(playhead) / sampleRate) * beatsPerSecond;
        
        musicalPos.bar = static_cast<int32_t>(totalBeats / timeSig.numerator) + 1;
        musicalPos.beat = static_cast<int32_t>(std::fmod(totalBeats, timeSig.numerator)) + 1;
        
        double subBeat = std::fmod(totalBeats, 1.0);
        musicalPos.tick = static_cast<int32_t>(subBeat * MusicalTime::kTicksPerBeat);
    }
};

} // namespace Aura::Core::Engine
