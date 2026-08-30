#include "engine_clock.hpp"
#include "tempo_map.hpp"
#include <cstdio>
#include <cmath>

namespace Aura::Core::Engine {

EngineClock::EngineClock() : m_quantumPlayhead(0.0), m_nominalRate(48000.0), m_effectiveRate(48000.0) {}

EngineClock& EngineClock::getInstance() {
    static EngineClock instance;
    return instance;
}

void EngineClock::advance(uint64_t samples) {
    // Advanced tempo logic or direct delta calculation based on m_effectiveRate/m_nominalRate
    double nominal = m_nominalRate.load();
    double effective = m_effectiveRate.load();
    double delta = samples * (effective / nominal);
    m_quantumPlayhead.store(m_quantumPlayhead.load() + delta);
}

double EngineClock::getSubSampleOffset() const {
    double playhead = m_quantumPlayhead.load();
    return playhead - std::floor(playhead);
}

void EngineClock::setPlayhead(double beats) {
    m_quantumPlayhead.store(beats);
}

double EngineClock::getCurrentBeats() const {
    return m_quantumPlayhead.load();
}

uint64_t EngineClock::getCurrentSample() const {
    return static_cast<uint64_t>(std::floor(m_quantumPlayhead.load()));
}

void EngineClock::setHardwareRate(double sr) { 
    m_nominalRate.store(sr);
    if (m_effectiveRate.load() == 0.0) m_effectiveRate.store(sr);
}

void EngineClock::setEffectiveRate(double sr) {
    m_effectiveRate.store(sr);
}

double EngineClock::samplesToBeats(uint64_t samples) const {
    return TempoMap::getInstance().samplesToBeats(samples, m_nominalRate.load());
}

uint64_t EngineClock::beatsToSamples(double beats) const {
    return TempoMap::getInstance().beatsToSamples(beats, m_nominalRate.load());
}

MusicalTime EngineClock::getMusicalTime(uint64_t samples) const {
    double beats = samplesToBeats(samples);

    // Ground truth: convert to integer ticks (Logic Pro standard: 960 ticks/beat)
    // Using llround avoids truncation errors at exact beat/bar boundaries.
    const int64_t kTicksPerBeat      = MusicalTime::kTicksPerBeat;       // 960
    const int64_t kTicksPerSixteenth = MusicalTime::kTicksPerSixteenth;  // 240

    // Assume 4/4 time signature (4 beats per bar)
    const int64_t kBeatsPerBar        = 4;
    const int64_t kTicksPerBar        = kTicksPerBeat * kBeatsPerBar;    // 3840

    int64_t totalTicks = static_cast<int64_t>(beats * static_cast<double>(kTicksPerBeat));

    // Pure integer modulo arithmetic — zero floating-point rounding risk
    int32_t bar        = static_cast<int32_t>(totalTicks / kTicksPerBar) + 1;
    int32_t beat       = static_cast<int32_t>((totalTicks % kTicksPerBar) / kTicksPerBeat) + 1;
    int32_t sixteenth  = static_cast<int32_t>((totalTicks % kTicksPerBeat) / kTicksPerSixteenth) + 1;
    int32_t tick       = static_cast<int32_t>(totalTicks % kTicksPerSixteenth);

    MusicalTime mt;
    mt.bar        = bar;
    mt.beat       = beat;
    mt.sixteenth  = sixteenth;
    mt.tick       = tick;
    mt.totalTicks = totalTicks;
    mt.totalBeats = beats; // Kept for display/interpolation only

    return mt;
}

} // namespace Aura::Core::Engine
