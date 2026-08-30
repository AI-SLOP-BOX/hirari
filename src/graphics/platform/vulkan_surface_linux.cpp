#if defined(__linux__) && __has_include(<X11/Xlib.h>)
#define VK_USE_PLATFORM_XLIB_KHR 1
#include <X11/Xlib.h>
#endif

#include "vulkan_surface.hpp"

#if defined(__linux__) && defined(AURA_ENABLE_VULKAN) && AURA_ENABLE_VULKAN && defined(VK_USE_PLATFORM_XLIB_KHR)
namespace Aura::Graphics::Platform {

bool createVulkanSurfaceForNativeView(VkInstance instance, void* nativeView,
                                      VkSurfaceKHR* surface, std::string& error) {
    if (instance == VK_NULL_HANDLE || nativeView == nullptr || surface == nullptr) {
        error = "X11 Vulkan surface requires a valid instance, handle, and output";
        return false;
    }
    const auto* handle = static_cast<const VulkanX11Window*>(nativeView);
    if (handle->display == nullptr || handle->window == 0) {
        error = "X11 Vulkan surface received an invalid Display or Window";
        return false;
    }
    VkXlibSurfaceCreateInfoKHR createInfo{};
    createInfo.sType = VK_STRUCTURE_TYPE_XLIB_SURFACE_CREATE_INFO_KHR;
    createInfo.dpy = static_cast<Display*>(handle->display);
    createInfo.window = static_cast<Window>(handle->window);
    auto createSurface = reinterpret_cast<PFN_vkCreateXlibSurfaceKHR>(
        vkGetInstanceProcAddr(instance, "vkCreateXlibSurfaceKHR"));
    if (!createSurface || createSurface(instance, &createInfo, nullptr, surface) != VK_SUCCESS) {
        error = "vkCreateXlibSurfaceKHR failed";
        return false;
    }
    return true;
}

} // namespace Aura::Graphics::Platform
#else
// Linux builds without X11 headers still provide a deterministic fallback so
// the backend links cleanly. A future Wayland host can pass its own surface
// through initializeWithSurface().
#if defined(__linux__) && defined(AURA_ENABLE_VULKAN) && AURA_ENABLE_VULKAN
namespace Aura::Graphics::Platform {
bool createVulkanSurfaceForNativeView(VkInstance, void*, VkSurfaceKHR*, std::string& error) {
    error = "Linux Vulkan native surface requires X11 or an explicit Wayland surface";
    return false;
}
} // namespace Aura::Graphics::Platform
#endif
#endif
