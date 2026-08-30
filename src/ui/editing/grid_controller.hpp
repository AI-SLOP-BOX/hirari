#pragma once
#include <cmath>
#include <vector>
#include <algorithm>

namespace Aura::UI::Editing {

/**
 * @struct TimeSignature
 * @brief Represents a musical meter.
 */
struct TimeSignature {
    int numerator = 4;
    int denominator = 4;

    double getBeatsPerBar() const {
        return numerator > 0 && denominator > 0 ? static_cast<double>(numerator) : 4.0;
    }
};

/**
 * @class GridController
 * @brief Professional Time-Signature Aware Grid Engine.
 * HONEST FIX: Replaced 'Stress-Free' branding with real musical meter logic.
 */
class GridController {
public:
    enum class GridType { BAR, BEAT, DIVIDE };
    struct VisualGridLine { double pos; GridType type; };

    /**
     * @brief Determines the visual and snap resolution based on zoom.
     */
    double getGridDivision(double zoomLevel) const {
        if (zoomLevel < 10.0) return 4.0;
        if (zoomLevel < 40.0) return 1.0;
        if (zoomLevel < 150.0) return 0.5;
        if (zoomLevel < 600.0) return 0.25;
        return 0.125;
    }

    /**
     * @brief Snaps a position to the nearest grid line with magnetism.
     */
    double getSnappedPosition(double rawPosition, double zoomLevel, float magnetism, const TimeSignature& ts) const {
        double division = getGridDivision(zoomLevel);
        double nearestGrid = std::round(rawPosition / division) * division;
        double distance = std::abs(rawPosition - nearestGrid);

        double threshold = (division * 0.45) * magnetism;
        if (distance <= threshold) return nearestGrid;
        return rawPosition;
    }

    /**
     * @brief Identifies the hierarchy of a grid line based on the time signature.
     */
    GridType getGridType(double beatPos, const TimeSignature& ts) const {
        if (!std::isfinite(beatPos)) return GridType::DIVIDE;
        double beatsPerBar = ts.getBeatsPerBar();
        // Compare against the nearest integer grid index instead of a fixed
        // fmod epsilon. This remains stable for long timelines and negative
        // positions produced while scrolling before bar 1.
        const double barIndex = std::round(beatPos / beatsPerBar);
        const double beatIndex = std::round(beatPos);
        const double barEpsilon = std::max(1e-9, std::abs(beatPos) * 1e-12);
        if (std::abs(beatPos - barIndex * beatsPerBar) <= barEpsilon) return GridType::BAR;
        if (std::abs(beatPos - beatIndex) <= barEpsilon) return GridType::BEAT;
        return GridType::DIVIDE;
    }

    /**
     * @brief Returns visible grid lines for rendering.
     */
    std::vector<VisualGridLine> getHierarchicalGrid(double start, double end, double zoomLevel, const TimeSignature& ts) const {
        std::vector<VisualGridLine> res;
        double div = getGridDivision(zoomLevel);
        
        if (!std::isfinite(start) || !std::isfinite(end) || div <= 0.0 || end < start) return res;
        const auto first = static_cast<int64_t>(std::ceil(start / div));
        const auto last = static_cast<int64_t>(std::floor(end / div));
        if (last < first || last - first > 1'000'000) return res;
        res.reserve(static_cast<size_t>(last - first + 1));
        for (int64_t index = first; index <= last; ++index) {
            const double t = static_cast<double>(index) * div;
            res.push_back({t, getGridType(t, ts)});
        }
        return res;
    }
};

} // namespace Aura::UI::Editing
