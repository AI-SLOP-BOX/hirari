#pragma once
#include <cstdint>
#include <vector>

namespace Hirari::Core {

/// Control-surface automation point carried by tracks and the native project
/// adapter. The layout is part of the persisted `ARUA` track section.
struct ProjectAutomationPoint {
    double time = 0.0;
    float value = 0.0f;
    float curve = 0.0f;
};

/// One plugin-parameter automation lane.
struct ProjectPluginAutomationLane {
    uint32_t pluginIndex = 0;
    uint32_t parameterId = 0;
    std::vector<ProjectAutomationPoint> points;
};

} // namespace Hirari::Core
