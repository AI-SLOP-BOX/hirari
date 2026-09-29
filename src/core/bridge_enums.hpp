#pragma once
#include <cstdint>

namespace Hirari::Core::BridgeFFI {

enum class EngineCommand : uint32_t {
    Volume = 0,
    Pan = 1,
    Solo = 2,
    Mute = 3,
    AutomationToggle = 4,
    PluginBypass = 5,
    SpatialMode = 6,
    RestorationIntensity = 7
};

} // namespace Hirari::Core::BridgeFFI
