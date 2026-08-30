#pragma once

#include <stdint.h>
#include <vector>
#include <map>
#include <memory>
#include <mutex>
#include <algorithm>
#include <cmath>

namespace Aura::Core::Engine {

/**
 * @struct AutomationLane
 * @brief Complex spline-based automation curve for a single parameter.
 */
struct AutomationLane {
    struct Point {
        double time;   // In seconds
        float value;
        float curvature; // -1.0 to 1.0 (Bezier tension)
    };
    uint32_t paramId;
    std::vector<Point> points;

    float sampleAt(double time) const {
        if (points.empty()) return 0.0f;
        if (!std::isfinite(time)) return points.front().value;
        auto it = std::lower_bound(points.begin(), points.end(), time, 
            [](const Point& p, double t) { return p.time < t; });

        if (it == points.begin()) return points[0].value;
        if (it == points.end()) return points.back().value;

        const auto& p0 = *(it - 1);
        const auto& p1 = *it;
        const double span = p1.time - p0.time;
        if (!(span > 0.0) || !std::isfinite(span)) return p1.value;
        double f = std::clamp((time - p0.time) / span, 0.0, 1.0);
        
        // Bezier/Curvature interpolation
        float t = static_cast<float>(f);
        float tension = std::abs(p0.curvature);
        float curvedT = (p0.curvature > 0) ? std::pow(t, 1.0f + tension * 4.0f) : 1.0f - std::pow(1.0f - t, 1.0f + tension * 4.0f);
        
        return p0.value + curvedT * (p1.value - p0.value);
    }
};

/**
 * @class MasterAutomationOrchestrator
 * @brief High-density global automation management engine.
 */
class MasterAutomationOrchestrator {
public:
    void addLane(uint32_t trackId, const AutomationLane& lane) {
        AutomationLane sanitized = lane;
        sanitized.points.erase(std::remove_if(sanitized.points.begin(), sanitized.points.end(),
            [](const AutomationLane::Point& p) {
                return !std::isfinite(p.time) || !std::isfinite(p.value) ||
                       !std::isfinite(p.curvature);
            }), sanitized.points.end());
        std::stable_sort(sanitized.points.begin(), sanitized.points.end(),
            [](const AutomationLane::Point& a, const AutomationLane::Point& b) {
                return a.time < b.time;
            });
        std::lock_guard<std::mutex> lock(m_mutex);
        auto& lanes = m_tracks[trackId];
        auto it = std::find_if(lanes.begin(), lanes.end(),
            [&](const AutomationLane& existing) { return existing.paramId == sanitized.paramId; });
        if (it == lanes.end()) lanes.push_back(std::move(sanitized));
        else *it = std::move(sanitized);
    }

    float getParameterValue(uint32_t trackId, uint32_t paramId, double time) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto track = m_tracks.find(trackId);
        if (track == m_tracks.end()) return 0.0f;
        const auto lane = std::find_if(track->second.begin(), track->second.end(),
            [paramId](const AutomationLane& candidate) { return candidate.paramId == paramId; });
        return lane == track->second.end() ? 0.0f : lane->sampleAt(time);
    }

private:
    mutable std::mutex m_mutex;
    std::map<uint32_t, std::vector<AutomationLane>> m_tracks;
};

} // namespace Aura::Core::Engine
