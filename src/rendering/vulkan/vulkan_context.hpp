#pragma once

#include <vulkan/vulkan.h>
#include <algorithm>
#include <array>
#include <cstring>
#include <iterator>
#include <limits>
#include <vector>
#include <memory>
#include <string>
#include <cmath>

#if __has_include("aura_vulkan_ui_spv.hpp")
#include "aura_vulkan_ui_spv.hpp"
#define AURA_HAS_EMBEDDED_VULKAN_UI_SPV 1
#else
#define AURA_HAS_EMBEDDED_VULKAN_UI_SPV 0
#endif

namespace Aura::Rendering::Vulkan {

/**
 * @class VulkanContext
 * @brief Vulkan device and surface lifecycle state.
 */
    #include "vulkan_context_part_1.inc"
    #include "vulkan_context_part_2.inc"

} // namespace Aura::Rendering::Vulkan
