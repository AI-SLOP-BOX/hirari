#pragma once

#include <stdint.h>
#include <vector>
#include <string>
#include <memory>
#include <atomic>
#include <algorithm>

namespace Aura::Core::Engine {

/**
 * @struct Lane
 * @brief Definition of a vertical lane within a track (e.g., for Automation or Takes).
 */
struct Lane {
    uint32_t id;
    std::string name;
    bool visible = true;
    bool muted = false;
};

/**
 * @class MultiLaneManager
 * @brief Manages industrial-scale vertical orchestration within a single track.
 */
class MultiLaneManager {
public:
    void addLane(const Lane& lane) {
        for (auto& existing : m_lanes) {
            if (existing.id == lane.id) { existing = lane; return; }
        }
        m_lanes.push_back(lane);
    }

    void setLaneMuted(uint32_t laneId, bool muted) {
        for (auto& lane : m_lanes) if (lane.id == laneId) { lane.muted = muted; return; }
    }

    const std::vector<Lane>& getLanes() const { return m_lanes; }

    std::vector<Lane> copyLanes() const { return m_lanes; }

    bool removeLane(uint32_t laneId) {
        const auto it = std::find_if(m_lanes.begin(), m_lanes.end(),
            [laneId](const Lane& lane) { return lane.id == laneId; });
        if (it == m_lanes.end()) return false;
        m_lanes.erase(it);
        return true;
    }

    /**
     * @brief Resolve which lanes should be active for a given time block.
     */
    void resolveActiveLanes(uint64_t start, uint64_t end, std::vector<uint32_t>& activeIds) {
        (void)start; (void)end;
        activeIds.clear();
        for (const auto& lane : m_lanes) if (lane.visible && !lane.muted) activeIds.push_back(lane.id);
    }

private:
    std::vector<Lane> m_lanes;
};

} // namespace Aura::Core::Engine
