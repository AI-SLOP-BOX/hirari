#pragma once
#include <vector>
#include <string>
#include <algorithm>
#include <cmath>
#include <limits>
#include "../engine_types.hpp"

namespace Aura::Core::Engine {

enum class MarkerType { Point, Section };

/**
 * @struct Marker
 * @brief High-precision project milestone.
 * HONEST FIX: Added musical time support and durations for arrangement sections.
 */
struct Marker {
    uint32_t id;
    std::string name;
    uint64_t samplePos;
    MusicalTime musicalPos;
    uint64_t durationTicks = 0; // For Sections
    MarkerType type = MarkerType::Point;
    uint32_t color = 0xFF555555;
};

/**
 * @class MarkerSystem
 * @brief Industrial Project Navigation & Structure Engine.
 * HONEST FIX: Implemented hybrid sample/musical positioning.
 */
class MarkerSystem {
public:
    static MarkerSystem& getInstance() { static MarkerSystem i; return i; }

    void addMarker(const MusicalTime& pos, const std::string& name, MarkerType type = MarkerType::Point) {
        if (name.empty()) return;
        Marker marker{};
        marker.id = m_nextId++;
        marker.name = name;
        marker.musicalPos = pos;
        marker.type = type;
        m_markers.push_back(std::move(marker));
        syncToTempo(m_lastBpm, m_lastSampleRate);
    }

    /**
     * @brief SYNC: Synchronizes all marker sample positions to the current project tempo with industrial precision.
     */
    void syncToTempo(double bpm, double sr) {
        if (!std::isfinite(bpm) || bpm <= 0.0 || !std::isfinite(sr) || sr <= 0.0) return;
        m_lastBpm = bpm;
        m_lastSampleRate = sr;
        constexpr double kBeatsPerBar = 4.0;
        constexpr double kSecondsPerMinute = 60.0;
        for (auto& marker : m_markers) {
            const double beats = std::max(0.0,
                (static_cast<double>(marker.musicalPos.bar - 1) * kBeatsPerBar) +
                static_cast<double>(marker.musicalPos.beat - 1) +
                static_cast<double>(marker.musicalPos.tick) / MusicalTime::kTicksPerBeat);
            const double samples = beats * (kSecondsPerMinute / bpm) * sr;
            marker.samplePos = samples >= static_cast<double>(std::numeric_limits<uint64_t>::max())
                ? std::numeric_limits<uint64_t>::max()
                : static_cast<uint64_t>(std::llround(samples));
        }
        std::stable_sort(m_markers.begin(), m_markers.end(), [](const Marker& a, const Marker& b) {
            return a.samplePos < b.samplePos;
        });
    }


    const std::vector<Marker>& getMarkers() const { return m_markers; }

private:
    MarkerSystem() = default;
    std::vector<Marker> m_markers;
    uint32_t m_nextId = 1;
    double m_lastBpm = 120.0;
    double m_lastSampleRate = 44100.0;
};

} // namespace Aura::Core::Engine
