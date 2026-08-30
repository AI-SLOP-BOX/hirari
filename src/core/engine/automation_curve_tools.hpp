#pragma once

#include <algorithm>
#include <cmath>
#include <vector>

namespace Aura::Core::Engine {

template <typename Point>
class AutomationCurveTools {
public:
    static bool valid(const std::vector<Point>& points) {
        if (points.empty()) return true;
        for (size_t i = 0; i < points.size(); ++i) {
            if (!std::isfinite(points[i].beat) || !std::isfinite(points[i].value)) return false;
            if (i && points[i].beat < points[i - 1].beat) return false;
        }
        return true;
    }

    static void trim(std::vector<Point>& points, double start, double end) {
        if (!valid(points) || !std::isfinite(start) || !std::isfinite(end) || end < start) return;
        std::vector<Point> out;
        for (auto p : points) if (p.beat >= start && p.beat <= end) { p.beat -= start; out.push_back(p); }
        points.swap(out);
    }

    static void scale(std::vector<Point>& points, double beatOrigin, double beatFactor,
                      float valueOrigin, float valueFactor) {
        if (!valid(points) || !std::isfinite(beatOrigin) || !std::isfinite(beatFactor) ||
            !std::isfinite(valueOrigin) || !std::isfinite(valueFactor) || beatFactor <= 0.0) return;
        for (auto& p : points) { p.beat = beatOrigin + (p.beat - beatOrigin) * beatFactor;
            p.value = valueOrigin + (p.value - valueOrigin) * valueFactor; }
        std::sort(points.begin(), points.end(), [](const auto& a, const auto& b){ return a.beat < b.beat; });
    }

    static void reverse(std::vector<Point>& points, double start, double end) {
        if (!valid(points) || !std::isfinite(start) || !std::isfinite(end) || end < start) return;
        for (auto& p : points) p.beat = start + end - p.beat;
        std::sort(points.begin(), points.end(), [](const auto& a, const auto& b){ return a.beat < b.beat; });
    }

    static void clampValues(std::vector<Point>& points, float minimum, float maximum) {
        if (!std::isfinite(minimum) || !std::isfinite(maximum) || maximum < minimum) return;
        for (auto& p : points) p.value = std::clamp(p.value, minimum, maximum);
    }
};
}
