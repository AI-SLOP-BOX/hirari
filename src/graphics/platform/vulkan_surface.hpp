#pragma once

#if defined(AURA_ENABLE_VULKAN) && AURA_ENABLE_VULKAN
#if defined(__APPLE__)
#define VK_USE_PLATFORM_METAL_EXT 1
#elif defined(_WIN32)
#define VK_USE_PLATFORM_WIN32_KHR 1
#elif defined(__linux__) && __has_include(<X11/Xlib.h>)
#define VK_USE_PLATFORM_XLIB_KHR 1
#endif
#include <vulkan/vulkan.h>
#include <string>

namespace Aura::Graphics::Platform {

#if defined(__linux__)
struct VulkanX11Window {
    void* display = nullptr;
    uint64_t window = 0;
};
#endif

// The platform owns the native view and the Vulkan instance. The returned
// surface is owned by the caller and must be destroyed before the instance.
bool createVulkanSurfaceForNativeView(VkInstance instance, void* nativeView,
                                      VkSurfaceKHR* surface, std::string& error);

} // namespace Aura::Graphics::Platform
#endif
