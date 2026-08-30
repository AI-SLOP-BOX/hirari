#import <Cocoa/Cocoa.h>
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>

#include "vulkan_surface.hpp"

#if defined(AURA_ENABLE_VULKAN) && AURA_ENABLE_VULKAN
namespace Aura::Graphics::Platform {

bool createVulkanSurfaceForNativeView(VkInstance instance, void* nativeView,
                                      VkSurfaceKHR* surface, std::string& error) {
    if (instance == VK_NULL_HANDLE || nativeView == nullptr || surface == nullptr) {
        error = "MoltenVK surface requires a valid instance, native view, and output";
        return false;
    }
    NSView* view = (__bridge NSView*)nativeView;
    if (!view) {
        error = "MoltenVK surface received a null NSView";
        return false;
    }
    if (!view.wantsLayer) view.wantsLayer = YES;
    CAMetalLayer* layer = (CAMetalLayer*)view.layer;
    if (!layer) {
        error = "Native NSView has no CAMetalLayer";
        return false;
    }
    auto createSurface = reinterpret_cast<PFN_vkCreateMetalSurfaceEXT>(
        vkGetInstanceProcAddr(instance, "vkCreateMetalSurfaceEXT"));
    if (!createSurface) {
        error = "Vulkan instance does not expose vkCreateMetalSurfaceEXT";
        return false;
    }
    VkMetalSurfaceCreateInfoEXT createInfo{};
    createInfo.sType = VK_STRUCTURE_TYPE_METAL_SURFACE_CREATE_INFO_EXT;
    createInfo.pLayer = layer;
    const VkResult result = createSurface(instance, &createInfo, nullptr, surface);
    if (result != VK_SUCCESS) {
        error = "vkCreateMetalSurfaceEXT failed";
        return false;
    }
    return true;
}

} // namespace Aura::Graphics::Platform
#endif
