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

#if __has_include("hirari_vulkan_ui_spv.hpp")
#include "hirari_vulkan_ui_spv.hpp"
#define HIRARI_HAS_EMBEDDED_VULKAN_UI_SPV 1
#else
#define HIRARI_HAS_EMBEDDED_VULKAN_UI_SPV 0
#endif

namespace Hirari::Rendering::Vulkan {

/**
 * @class VulkanContext
 * @brief Vulkan device and surface lifecycle state.
 */
    #include "vulkan_context_part_1.inc"
    #include "vulkan_context_part_2.inc"

} // namespace Hirari::Rendering::Vulkan
