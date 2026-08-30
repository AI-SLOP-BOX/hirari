#include "vulkan_kernel.hpp"
#include <algorithm>
#include <iostream>
#include <cstring>
#include <limits>
#include <utility>
#include <cmath>
#if defined(AURA_ENABLE_VULKAN) && AURA_ENABLE_VULKAN
#include <vulkan/vulkan.h>
#include "../../rendering/vulkan/vulkan_context.hpp"
#endif

namespace Aura::Graphics::Platform {
#include "vulkan_kernel_part_1.inc"
#include "vulkan_kernel_part_2.inc"
} // namespace Aura::Graphics::Platform
