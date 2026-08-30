#pragma once
#include "../graphics_kernel.hpp"
#include <cstdint>
#include <string>
#include <vector>
#include <atomic>
#include <mutex>
#include <chrono>
#include <array>

#if defined(AURA_ENABLE_VULKAN) && AURA_ENABLE_VULKAN
#if defined(__APPLE__)
#define VK_USE_PLATFORM_METAL_EXT 1
#endif
#include <vulkan/vulkan.h>
#include "../../rendering/vulkan/vulkan_context.hpp"
#include "vulkan_surface.hpp"
#endif

namespace Aura::Graphics::Platform {

/**
 * @class VulkanGraphicsKernel
 * @brief High-performance Vulkan implementation for Windows/Linux.
 * Safe bootstrap implementation; drawing is deferred until a swapchain command
 * buffer is provided by the platform integration.
 */
class VulkanGraphicsKernel : public IGraphicsKernel {
public:
    VulkanGraphicsKernel();
    virtual ~VulkanGraphicsKernel();

    bool initialize(void* nativeWindowHandle) override;
#if defined(AURA_ENABLE_VULKAN) && AURA_ENABLE_VULKAN
    // Platform layers create the VkSurfaceKHR because only they know whether
    // the host window is Win32, X11, Wayland, Cocoa, or MoltenVK-backed.
    bool initializeWithSurface(VkInstance instance, VkSurfaceKHR surface,
                               uint32_t width, uint32_t height);
#endif
    void beginFrame() override;
    void endFrame() override;

    void setScale(float s) override { m_scale = s; }
    float getScale() const override { return m_scale; }

    void updateSpectrogram(const std::vector<float>& data) override;
    void pushScissor(float x, float y, float w, float h) override;
    void popScissor() override;
    void drawWaveformPath(const float*, const float*, size_t, float, float, float, float, uint32_t) override;
    void drawMeter(uint32_t trackId, float levelL, float levelR, float x, float y, float w, float h) override;

    void drawRoundedRect(float x, float y, float w, float h, float radius, uint32_t color) override;
    void drawRect(float x, float y, float w, float h, uint32_t color) override { drawRoundedRect(x, y, w, h, 0.0f, color); }
    void drawGradientRect(float x, float y, float w, float h, uint32_t colorTop, uint32_t colorBottom) override;
    void drawText(const std::string& text, float x, float y, float size, uint32_t color) override;
    float measureText(const std::string& text, float size) const override;
    void drawCircle(float x, float y, float radius, uint32_t color) override;
    void drawLine(float x1, float y1, float x2, float y2, float thickness, uint32_t color) override;
    void drawFilledPath(const std::vector<float>& points, uint32_t color) override;
    void drawDropShadow(float x, float y, float w, float h, float radius, uint32_t color) override;
    void applyBlurEffect(float x, float y, float w, float h, float intensity) override;
    void drawBezierPath(const std::vector<float>& points, uint32_t color, float thickness) override;
    void drawArc(float cx, float cy, float radius, float start, float end, float thickness, uint32_t color) override;
    void drawVertexPath(const Vertex* vertices, size_t count, float thickness) override;

    void drawBrushedCircle(float x, float y, float radius, uint32_t color) override;
    void drawVertexPathFilled(const Vertex* vertices, size_t count) override;
    void drawFilledTriangle(float x1, float y1, float x2, float y2, float x3, float y3, uint32_t color) override;
    void drawNeonRect(float x, float y, float w, float h, float radius, float glow, uint32_t color) override;
    void drawGlassRect(float x, float y, float w, float h, float radius, uint32_t color) override { drawRoundedRect(x, y, w, h, radius, color); }
    void drawBezierCurve(float x1, float y1, float cp1x, float cp1y, float cp2x, float cp2y, float x2, float y2, float thickness, uint32_t color) override;
    void calculateBezier(float x1, float y1, float cp1x, float cp1y, float cp2x, float cp2y, float x2, float y2, float t, float& ox, float& oy) const override;
    void drawNative(float x, float y, float w, float h, float p1, float p2, float p3, uint32_t c1, uint32_t c2, float type) override;
    RenderingTelemetry getTelemetry() const override;
    
    void drawIconAudio(float x, float y, float s, uint32_t col) override;
    void drawIconInstrument(float x, float y, float s, uint32_t col) override;
    void drawIconMidi(float x, float y, float s, uint32_t col) override;
    void drawIconMic(float x, float y, float s, uint32_t col) override;
    void drawIconDrums(float x, float y, float s, uint32_t col) override;
    
    void drawTriangle(float x1, float y1, float x2, float y2, float x3, float y3, float thickness, uint32_t color) override;
    void drawGoniometer(float x, float y, float w, float h, const float* historyL, const float* historyR, size_t count) override;

    std::string getBackendName() const override {
        return m_available ? "Vulkan" : "Vulkan unavailable (fallback required)";
    }

    // These capability queries are intentionally additive; existing callers
    // can continue to use IGraphicsKernel without API changes.
    bool isAvailable() const noexcept { return m_available; }
    bool isContextReady() const noexcept { return m_contextReady; }
    const char* lastError() const noexcept { return m_lastError.c_str(); }

    struct SDFPushConstants {
        float x, y, w, h;
        uint32_t colorL, colorR, colorT, colorB; // Packed RGBA
        float radius;
        float type; // 0=Rect, 1=Arc, 2=MSDF
        float border;
        float glow;
    };

private:
    void cleanup();
    void pushSDF(const SDFPushConstants& constants);
    
    float m_scale = 1.0f;
#if defined(AURA_ENABLE_VULKAN) && AURA_ENABLE_VULKAN
    VkInstance m_instance = VK_NULL_HANDLE;
    VkPhysicalDevice m_physicalDevice = VK_NULL_HANDLE;
    VkDevice m_device = VK_NULL_HANDLE;
    VkQueue m_graphicsQueue = VK_NULL_HANDLE;
    VkSurfaceKHR m_surface = VK_NULL_HANDLE;
    VkPipelineLayout m_pipelineLayout = VK_NULL_HANDLE;
#else
    void* m_instance = nullptr;
    void* m_physicalDevice = nullptr;
    void* m_device = nullptr;
    void* m_graphicsQueue = nullptr;
    void* m_surface = nullptr;
    void* m_pipelineLayout = nullptr;
#endif
    bool m_available = false;
    bool m_runtimeReady = false;
    bool m_contextReady = false;
    std::string m_lastError = "Vulkan backend not initialized";
    uint32_t m_graphicsQueueFamily = UINT32_MAX;
    // Control-rate staging cache. The render backend can upload this to a
    // sampled image when the texture path is available; until then, input is
    // still validated and retained instead of silently discarded.
    mutable std::mutex m_spectrogramMutex;
    std::vector<float> m_spectrogramData;
    std::atomic<uint64_t> m_spectrogramRevision{0};
    std::atomic<uint32_t> m_drawCallCount{0};
    std::atomic<uint32_t> m_vertexCount{0};
    std::atomic<float> m_fps{0.0f};
    std::atomic<uint64_t> m_lastFrameMicros{0};
    struct Scissor { float x, y, w, h; };
    std::array<Scissor, 16> m_scissors{};
    size_t m_scissorDepth = 0;
};

} // namespace Aura::Graphics::Platform
