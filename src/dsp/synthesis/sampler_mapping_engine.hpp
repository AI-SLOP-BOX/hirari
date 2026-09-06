#pragma once

#include <algorithm>
#include <cstdint>
#include <string>
#include <utility>
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
        // Prefer the most specific zone.  A full-range fallback registered
        // before a close-mic velocity layer must not shadow that layer.
        const auto zone = std::min_element(m_zones.begin(), m_zones.end(),
            [note, velocity](const SamplerZone& lhs, const SamplerZone& rhs) {
                const auto matches = [note, velocity](const SamplerZone& candidate) {
                    return note >= candidate.lowNote && note <= candidate.highNote &&
                           velocity >= candidate.lowVel && velocity <= candidate.highVel;
                };
                const bool lhsMatches = matches(lhs);
                const bool rhsMatches = matches(rhs);
                if (lhsMatches != rhsMatches) return lhsMatches;
                if (!lhsMatches) return false;
                const uint32_t lhsSpan =
                    static_cast<uint32_t>(lhs.highNote - lhs.lowNote) * 128u +
                    static_cast<uint32_t>(lhs.highVel - lhs.lowVel);
                const uint32_t rhsSpan =
                    static_cast<uint32_t>(rhs.highNote - rhs.lowNote) * 128u +
                    static_cast<uint32_t>(rhs.highVel - rhs.lowVel);
                return lhsSpan < rhsSpan;
            });
        if (zone != m_zones.end() && note >= zone->lowNote && note <= zone->highNote &&
            velocity >= zone->lowVel && velocity <= zone->highVel) {
            return zone->samplePath;
        }
        return "";
    }

    /**
     * @brief Returns every matching layer in deterministic specificity order.
     * This lets a voice allocator choose round-robin or layered playback while
     * keeping the legacy single-sample resolver available to older clients.
     */
    std::vector<std::string> resolveSamples(uint8_t note, uint8_t velocity) const {
        std::vector<std::pair<uint32_t, std::string>> matches;
        for (const auto& zone : m_zones) {
            if (note < zone.lowNote || note > zone.highNote || velocity < zone.lowVel ||
                velocity > zone.highVel) {
                continue;
            }
            const uint32_t span = static_cast<uint32_t>(zone.highNote - zone.lowNote) * 128u +
                                  static_cast<uint32_t>(zone.highVel - zone.lowVel);
            matches.emplace_back(span, zone.samplePath);
        }
        std::stable_sort(matches.begin(), matches.end(),
                         [](const auto& lhs, const auto& rhs) { return lhs.first < rhs.first; });
        std::vector<std::string> result;
        result.reserve(matches.size());
        for (auto& match : matches) result.push_back(std::move(match.second));
        return result;
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
