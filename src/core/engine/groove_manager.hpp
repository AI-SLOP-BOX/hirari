#pragma once
#include <vector>
#include <string>
#include <algorithm>
#include "../engine_types.hpp"

namespace Aura::Core::Engine {

struct GroovePoint {
    int32_t tickOffset;
    float velocityMult = 1.0f;
    uint64_t sourceTick;

    bool operator<(const GroovePoint& other) const { return sourceTick < other.sourceTick; }
};

/**
 * @struct GrooveMap
 * @brief High-precision rhythmic template.
 */
struct GrooveMap {
    std::string name;
    std::vector<GroovePoint> points;
};

/**
 * @class GrooveManager
 * @brief Groove Extraction and Application Engine.
 * Replaced O(N*M) search with O(N log M) binary search.
 */
class GrooveManager {
public:
    static GrooveManager& getInstance() { static GrooveManager i; return i; }

    /**
     * @brief Applies a groove template to MIDI tick positions and velocities.
     */
    void applyGroove(uint64_t* tickPositions, float* velocities, size_t count, const GrooveMap& map, float strength = 1.0f) {
        if (count == 0 || map.points.empty()) return;

        // Assumes map.points is pre-sorted to satisfy RT-safety (no dynamic allocations or sorting in real-time)
        for (size_t i = 0; i < count; ++i) {
            uint64_t noteTick = tickPositions[i];

            // Use binary search to find the nearest groove point
            auto it = std::lower_bound(map.points.begin(), map.points.end(), noteTick, 
                [](const GroovePoint& pt, uint64_t tick) {
                    return pt.sourceTick < tick;
                });

            GroovePoint nearest;
            if (it == map.points.end()) {
                nearest = map.points.back();
            } else if (it == map.points.begin()) {
                nearest = map.points.front();
            } else {
                auto prev = it - 1;
                if ((it->sourceTick - noteTick) < (noteTick - prev->sourceTick)) {
                    nearest = *it;
                } else {
                    nearest = *prev;
                }
            }

            // Apply groove shift to tick position
            int64_t offset = static_cast<int64_t>(nearest.tickOffset * strength);
            int64_t newTick = static_cast<int64_t>(noteTick) + offset;
            tickPositions[i] = static_cast<uint64_t>(std::max(static_cast<int64_t>(0), newTick));

            // Apply groove dynamics to velocity
            if (velocities) {
                float vel = velocities[i];
                float targetVel = vel * nearest.velocityMult;
                velocities[i] = std::clamp(vel + strength * (targetVel - vel), 0.0f, 127.0f);
            }
        }
    }

private:
    GrooveManager() = default;
};

} // namespace Aura::Core::Engine
