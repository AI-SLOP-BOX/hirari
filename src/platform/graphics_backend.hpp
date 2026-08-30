#pragma once

namespace Aura::Platform {

enum class GraphicsBackend { Auto, Metal, Vulkan, Cpu };

inline GraphicsBackend preferredGraphicsBackend() noexcept {
#if defined(__APPLE__)
    return GraphicsBackend::Metal;
#else
    return GraphicsBackend::Cpu;
#endif
}

} // namespace Aura::Platform
