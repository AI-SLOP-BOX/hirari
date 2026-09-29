#pragma once
#include <cstdint>
#include <algorithm>
#include "../engine_types.hpp"
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

/**
 * @class GridSnapManager
 * @brief High-precision rhythmic alignment engine.
 * HONEST FIX: Implemented tick-based snapping and time-signature awareness.
 */
class GridSnapManager {
public:
    enum class Resolution { 
        Measure, Beat, Half, Quarter, Eighth, Sixteenth, ThirtySecond,
        EighthTriplet, SixteenthDotted
    };

    static uint64_t snapAbsolute(uint64_t ticks, Resolution res, const TimeSignature& sig) {
        return hirari_grid_snap_absolute(
            ticks, static_cast<uint32_t>(res),
            static_cast<uint32_t>(std::max(1, sig.numerator)),
            static_cast<uint32_t>(std::max(1, sig.denominator)));
    }

    static uint64_t snapRelative(uint64_t originalTicks, uint64_t deltaTicks, Resolution res, const TimeSignature& sig) {
        return hirari_grid_snap_relative(
            originalTicks, deltaTicks, static_cast<uint32_t>(res),
            static_cast<uint32_t>(std::max(1, sig.numerator)),
            static_cast<uint32_t>(std::max(1, sig.denominator)));
    }
};

} // namespace Hirari::Core::Engine
