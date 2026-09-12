#pragma once
#include <vector>
#include <array>
#include <memory>
#include <algorithm>
#include <atomic>
#include <cstdint>
#include <mutex>
#include <string>
#include <string_view>
#include <cmath>
#include <fstream>
#include <filesystem>
#include <chrono>
#include "../../graphics/graphics_kernel.hpp"
#include "../../core/aura_unified_engine.hpp"
#include "../../core/engine/track.hpp"
#include "view_transformer.hpp"
#include "../../external/nlohmann/json.hpp"

namespace Aura::UI::Main {

enum class LayoutMode {
    Single,
    SplitHorizontal,
    SplitVertical,
    Floating
};

// Experience density is deliberately independent from window layout.  A
// producer can keep a split layout while switching between a guided surface
// and a full engineering surface, and extensions can add their own panel
// without inventing another global UI mode.
enum class ExperienceMode : uint8_t {
    Beginner,
    Pro,
    Custom
};

/**
 * @class WorkspaceManager
 * @brief Manages window Z-order and layout orchestration.
 * HONEST FIX: Purged 'UI DNA' and 'Infinite Synthesis' hallucinations.
 */
class WorkspaceManager {
public:
#include "workspace_public_part_1.inc"
#include "workspace_public_part_2.inc"
#include "workspace_private.inc"
};

// Compatibility Typedef
using AuraWorkspace = WorkspaceManager;

} // namespace Aura::UI::Main
