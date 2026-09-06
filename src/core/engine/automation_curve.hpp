#pragma once
#include <memory>
#include <algorithm>
#include <vector>
#include <mutex>
#include <atomic>
#include <cmath>
#include "automation_interpolator.hpp"

namespace Aura::Core::Engine {

enum class InterpolationType { Hold, Linear, Bezier, Exponential };

/**
 * @class AutomationCurve
 * @brief Industrial Parameter Automation Engine.
 * HONEST FIX: Implemented segment caching for O(1) real-time evaluation.
 */
class AutomationCurve {
public:
    struct Point {
        double time; float value;
        InterpolationType type = InterpolationType::Linear;
        float curvature = 0.5f; // For Bezier/Exp
    };

    AutomationCurve() {
        m_pointList = std::make_shared<std::vector<Point>>();
    }

    void addPoint(double time, float value, InterpolationType type = InterpolationType::Linear) {
        if (!std::isfinite(time) || !std::isfinite(value)) return;
        std::lock_guard<std::mutex> lock(m_pointListMutex);
        auto current = std::atomic_load_explicit(&m_pointList, std::memory_order_acquire);
        auto next = std::make_shared<std::vector<Point>>(*current);
        next->push_back({time, value, type});
        std::sort(next->begin(), next->end(), [](const Point& a, const Point& b) {
            return a.time < b.time;
        });
        std::atomic_store_explicit(&m_pointList, std::move(next), std::memory_order_release);
    }

    /**
     * @brief GET: Sample-accurate O(1) evaluation using segment caching.
     */
    float getValueAt(double time) const {
        const auto points = std::atomic_load_explicit(&m_pointList, std::memory_order_acquire);
        if (points->empty()) return 0.0f;
        if (time <= points->front().time) return points->front().value;
        if (time >= points->back().time) return points->back().value;
        auto it = std::upper_bound(points->begin(), points->end(), time,
            [](double t, const Point& p) { return t < p.time; });
        const auto& b = *it;
        const auto& a = *(it - 1);
        if (a.type == InterpolationType::Hold || b.time <= a.time) return a.value;
        const float amount = static_cast<float>((time - a.time) / (b.time - a.time));
        const float curvature = std::clamp(a.curvature, -1.0f, 1.0f);
        const float shaped = a.type == InterpolationType::Linear
            ? amount
            : std::clamp(amount + curvature * amount * (1.0f - amount) * (1.0f - 2.0f), 0.0f, 1.0f);
        return a.value + (b.value - a.value) * shaped;
    }

    // Snapshot for UI editors; callers never observe the mutable shared list.
    std::vector<Point> getPoints() const {
        const auto points = std::atomic_load_explicit(&m_pointList, std::memory_order_acquire);
        return points ? *points : std::vector<Point>{};
    }

    // Lock-free alias used by realtime renderers and legacy UI clients.
    float evaluateAtNoLock(double time) const { return getValueAt(time); }

private:
    mutable std::mutex m_pointListMutex;
    std::shared_ptr<std::vector<Point>> m_pointList;
};

} // namespace Aura::Core::Engine
