#pragma once

#include <algorithm>
#include <cstdint>
#include <string>
#include <vector>

namespace Aura::Core::DSP::Synthesis {

/**
 * @brief SamplerZone: Defines a mapping between MIDI input and an audio file.
 * Iconic Logic Pro EXS24-style multi-layer mapping.
 */
struct SamplerZone {
    uint8_t lowNote, highNote;
    uint8_t lowVel, highVel;
    std::string samplePath;
};

/**
 * @brief SamplerMappingEngine: Orchestrates multi-layered sampler instruments.
 * Essential for professional piano, drum, and orchestral instrument libraries.
 */
class SamplerMappingEngine {
public:
    static SamplerMappingEngine& getInstance() {
        static SamplerMappingEngine instance;
        return instance;
    }

    /** @brief Finds the first registered zone covering the MIDI note and velocity. */
    std::string resolveSample(uint8_t note, uint8_t velocity) const {
        const auto zone = std::find_if(m_zones.begin(), m_zones.end(),
            [note, velocity](const SamplerZone& candidate) {
                return note >= candidate.lowNote && note <= candidate.highNote &&
                       velocity >= candidate.lowVel && velocity <= candidate.highVel;
            });
        if (zone != m_zones.end()) {
            return zone->samplePath;
        }
        return "";
    }

    /** @brief Registers a zone when both note and velocity ranges are valid. */
    void addZone(const SamplerZone& zone) {
        if (zone.lowNote > zone.highNote || zone.lowVel > zone.highVel ||
            zone.samplePath.empty()) {
            return;
        }
        m_zones.push_back(zone);
    }

    std::size_t zoneCount() const noexcept { return m_zones.size(); }

private:
    SamplerMappingEngine() = default;
    std::vector<SamplerZone> m_zones;
};

} // namespace Aura::Core::DSP::Synthesis
