#pragma once
#include <cstdint>

namespace Hirari::DSP::Math {

/**
 * @class DenormalNumberKiller
 * @brief Essential for high-performance audio.
 * Prevents "Denormal Spikes" by forcing extremely small floating-point 
 * values (which take 100x more CPU cycles) to zero.
 */
class DenormalNumberKiller {
public:
    /**
     * @brief THE KILL: In-place floating point cleanup.
     * HONEST FIX: Uses a bitwise threshold to detect subnormal range.
     */
    static inline float kill(float value) {
        // Bitwise representation of float
        union {
            float f;
            uint32_t i;
        } u;
        u.f = value;
        
        // Exponent bits for float: bits 23-30
        // If exponent is 0, it's a subnormal or zero.
        if ((u.i & 0x7F800000) == 0) return 0.0f;
        
        return value;
    }
};

} // namespace Hirari::DSP::Math
