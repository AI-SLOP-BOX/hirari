#pragma once

#include <atomic>
#include <cmath>

namespace Aura::Core::Engine {

/**
 * @brief MusicalClock: The 'Conductor' of the DAW.
 * Handles high-precision conversion between audio samples and musical beats/bars.
 */
class MusicalClock {
public:
    static MusicalClock& getInstance() { static MusicalClock i; return i; }

    void setTempo(double bpm) { m_tempo = bpm; }
    void setTimeSignature(uint32_t num, uint32_t denom) { 
        m_numerator = num; m_denominator = denom; 
    }

    /**
     * @brief CONVERT: Samples -> Beats.
     * INDUSTRIAL: Delegating temporal conversion to the Rust 'MusicalClockOrchestrator'.
     */
    double samplesToBeats(uint64_t samples, double sampleRate) const {
        const double tempo = m_tempo.load(std::memory_order_relaxed);
        if (!std::isfinite(sampleRate) || sampleRate <= 0.0 || !std::isfinite(tempo) || tempo <= 0.0) return 0.0;
        return static_cast<double>(samples) * tempo / (sampleRate * 60.0);
    }

    uint32_t getBar(double beats) const {
        const auto numerator = m_numerator.load(std::memory_order_relaxed);
        const auto denominator = m_denominator.load(std::memory_order_relaxed);
        if (!std::isfinite(beats) || beats < 0.0 || numerator == 0 || denominator == 0) return 1;
        const double beatsPerBar = static_cast<double>(numerator) * 4.0 / static_cast<double>(denominator);
        if (!std::isfinite(beatsPerBar) || beatsPerBar <= 0.0) return 1;
        return static_cast<uint32_t>(std::floor(beats / beatsPerBar)) + 1;
    }

private:
    MusicalClock() = default;
    std::atomic<double> m_tempo{120.0};
    std::atomic<uint32_t> m_numerator{4};
    std::atomic<uint32_t> m_denominator{4};
};


} // namespace Aura::Core::Engine
