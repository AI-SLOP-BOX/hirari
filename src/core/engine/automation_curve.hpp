#pragma once

#include <cstddef>
#include <cstdint>
#include <vector>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

enum class InterpolationType { Hold, Linear, Bezier, Exponential };

/** C++ compatibility API backed by Rust's immutable automation snapshots. */
class AutomationCurve {
public:
    struct Point {
        double time;
        float value;
        InterpolationType type = InterpolationType::Linear;
        float curvature = 0.5f;
    };

    AutomationCurve() : m_state(hirari_automation_curve_create()) {}
    ~AutomationCurve() { hirari_automation_curve_destroy(m_state); }
    AutomationCurve(const AutomationCurve&) = delete;
    AutomationCurve& operator=(const AutomationCurve&) = delete;
    AutomationCurve(AutomationCurve&&) = delete;
    AutomationCurve& operator=(AutomationCurve&&) = delete;

    void addPoint(double time, float value,
                  InterpolationType type = InterpolationType::Linear) {
        hirari_automation_curve_add_point(m_state, time, value,
                                          static_cast<int32_t>(type));
    }

    float getValueAt(double time) const {
        return hirari_automation_curve_value_at(m_state, time);
    }

    std::vector<Point> getPoints() const {
        size_t count = hirari_automation_curve_copy_points(m_state, nullptr, 0);
        std::vector<Point> points(count);
        while (true) {
            const size_t required = hirari_automation_curve_copy_points(
                m_state, points.data(), points.size());
            if (required <= points.size()) {
                points.resize(required);
                return points;
            }
            points.resize(required);
        }
    }

    float evaluateAtNoLock(double time) const { return getValueAt(time); }

private:
    void* m_state = nullptr;
};

static_assert(sizeof(AutomationCurve::Point) == 24);
static_assert(offsetof(AutomationCurve::Point, curvature) == 16);

} // namespace Hirari::Core::Engine
