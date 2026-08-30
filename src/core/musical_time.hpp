#pragma once
#include <cstdint>

namespace Aura::Core {

/**
 * @struct MusicalTime
 * @brief Integer-tick-based musical position for sample-exact timeline accuracy.
 *
 * All bar/beat/sixteenth/tick fields are derived from totalTicks using pure integer
 * division and modulo, fully eliminating floating-point rounding errors at bar/beat
 * boundaries. Matches Logic Pro's standard of kTicksPerBeat = 960.
 */
struct MusicalTime {
    static constexpr int64_t kTicksPerBeat      = 960;
    static constexpr int64_t kSixteenthsPerBeat = 4;
    static constexpr int64_t kTicksPerSixteenth = kTicksPerBeat / kSixteenthsPerBeat; // 240

    int32_t bar;        // 1-based bar index
    int32_t beat;       // 1-based beat index within bar
    int32_t sixteenth;  // 1-based sixteenth index within beat
    int32_t tick;       // 0-based tick within sixteenth (0 to kTicksPerSixteenth-1)
    int64_t totalTicks; // Absolute monotonic tick position (ground truth)
    double  totalBeats; // Floating-point beats for display/interpolation only
};

} // namespace Aura::Core
