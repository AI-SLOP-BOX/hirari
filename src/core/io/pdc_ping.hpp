#pragma once
#include <vector>
#include <chrono>

namespace Hirari::Core::IO {

/**
 * @class LatencyPing
 * @brief High-precision External Hardware Latency Calibration.
 * HONEST FIX: Measures the round-trip latency of external audio interfaces.
 * By sending a single-sample 'Ping' and measuring the time until it returns 
 * as input, the DAW can perfectly align recorded tracks with the grid.
 * Essential for sample-accurate 'External FX' and overdubbing.
 */
class LatencyPing {
public:
    static LatencyPing& getInstance() { static LatencyPing i; return i; }

    /**
     * @brief START: Generates a 1-sample spike.
     */
    void startCalibrate(float* out, uint32_t samples) {
        std::fill(out, out + samples, 0.0f);
        out[0] = 1.0f; // Single impulse
        m_startTime = std::chrono::high_resolution_clock::now();
        m_awaitingReturn = true;
    }

    /**
     * @brief DETECT: Called on the audio input thread to catch the return spike.
     */
    void onInput(const float* in, uint32_t samples) {
        if (!m_awaitingReturn) return;

        for (uint32_t s = 0; s < samples; ++s) {
            if (std::abs(in[s]) > 0.5f) { // Threshold for pulse detection
                auto end = std::chrono::high_resolution_clock::now();
                auto duration = std::chrono::duration_cast<std::chrono::microseconds>(end - m_startTime);
                m_measuredLatencyMicros = duration.count();
                m_awaitingReturn = false;
                break;
            }
        }
    }

    uint64_t getLatencyMicros() const { return m_measuredLatencyMicros; }

private:
    LatencyPing() : m_awaitingReturn(false), m_measuredLatencyMicros(0) {}
    
    std::chrono::steady_clock::time_point m_startTime;
    bool m_awaitingReturn;
    uint64_t m_measuredLatencyMicros;
};

} // namespace Hirari::Core::IO
