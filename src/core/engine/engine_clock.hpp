#pragma once
#include <atomic>
#include <cstdint>
#include <cmath>
#include <algorithm>
#include "../musical_time.hpp"

namespace Aura::Core::Engine {

class EngineClock {
public:
    static EngineClock& getInstance();
    
    void advance(uint64_t samples);
    double getSubSampleOffset() const;
    void setPlayhead(double beats);
    double getCurrentBeats() const;
    uint64_t getCurrentSample() const;
    void setHardwareRate(double sr);
    void setEffectiveRate(double sr);

    double samplesToBeats(uint64_t samples) const;
    uint64_t beatsToSamples(double beats) const;
    MusicalTime getMusicalTime(uint64_t samples) const;

private:
    EngineClock();
    
    std::atomic<double> m_quantumPlayhead;
    std::atomic<double> m_nominalRate;
    std::atomic<double> m_effectiveRate;
};

} // namespace Aura::Core::Engine
