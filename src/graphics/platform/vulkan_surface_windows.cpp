#include "vulkan_surface.hpp"

#if defined(_WIN32) && defined(AURA_ENABLE_VULKAN) && AURA_ENABLE_VULKAN
#define WIN32_LEAN_AND_MEAN
#include <windows.h>

namespace Aura::Graphics::Platform {

bool createVulkanSurfaceForNativeView(VkInstance instance, void* nativeView,
                                      VkSurfaceKHR* surface, std::string& error) {
    if (instance == VK_NULL_HANDLE || nativeView == nullptr || surface == nullptr) {
        error = "Win32 Vulkan surface requires a valid instance, HWND, and output";
        return false;
    }
    VkWin32SurfaceCreateInfoKHR createInfo{};
    createInfo.sType = VK_STRUCTURE_TYPE_WIN32_SURFACE_CREATE_INFO_KHR;
    createInfo.hwnd = static_cast<HWND>(nativeView);
    createInfo.hinstance = GetModuleHandleW(nullptr);
    if (createInfo.hinstance == nullptr) {
        error = "GetModuleHandleW failed while creating Vulkan surface";
        return false;
    }
    auto createSurface = reinterpret_cast<PFN_vkCreateWin32SurfaceKHR>(
        vkGetInstanceProcAddr(instance, "vkCreateWin32SurfaceKHR"));
    if (!createSurface || createSurface(instance, &createInfo, nullptr, surface) != VK_SUCCESS) {
        error = "vkCreateWin32SurfaceKHR failed";
        return false;
    }
    return true;
}

} // namespace Aura::Graphics::Platform
#endif
