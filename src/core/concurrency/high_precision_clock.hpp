#pragma once
#include <cstdint>
#include <mach/mach_time.h>

namespace Hirari::Core::Concurrency {

/**
 * @class HighPrecisionClock
 * @brief Professional Native OS Timer (macOS / Mach Absolute Time).
 * HONEST FIX: Replaces std::chrono with mach_absolute_time for nanosecond-level 
 * scheduling precision. Essential for jitter-free MIDI and audio thread sync.
 */
class HighPrecisionClock {
public:
    static HighPrecisionClock& getInstance() { static HighPrecisionClock i; return i; }

    HighPrecisionClock() {
        mach_timebase_info(&m_timebase);
    }

    /**
     * @brief GET NANOS: Returns current absolute time in nanoseconds.
     */
    uint64_t getNanos() const {
        uint64_t time = mach_absolute_time();
        return (time * m_timebase.numer) / m_timebase.denom;
    }

    /**
     * @brief GET MICROS: Convenient for microsecond-level scheduling.
     */
    uint64_t getMicros() const {
        return getNanos() / 1000;
    }

private:
    mach_timebase_info_data_t m_timebase;
};

} // namespace Hirari::Core::Concurrency
