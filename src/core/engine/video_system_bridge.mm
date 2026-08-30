#import "video_system_bridge.hpp"
#import "video_system.hpp"

namespace Aura::Core::Engine::VideoBridge {

bool requestFrameAt(double seconds) noexcept {
    try {
        return VideoSystem::getInstance().requestFrameAt(seconds);
    } catch (...) {
        return false;
    }
}

bool loadVideo(const char* path) noexcept {
    try {
        if (!path || !*path) return false;
        VideoSystem::getInstance().loadVideo(path);
        return VideoSystem::getInstance().hasVideo();
    } catch (...) {
        return false;
    }
}

bool copyCurrentFrame(std::vector<uint8_t>& pixels, uint32_t& width, uint32_t& height) noexcept {
    try {
        double seconds = 0.0;
        return VideoSystem::getInstance().copyCurrentFrame(pixels, width, height, seconds);
    } catch (...) {
        pixels.clear(); width = height = 0; return false;
    }
}

uint64_t frameRevision() noexcept {
    try {
        return VideoSystem::getInstance().frameRevision();
    } catch (...) {
        return 0;
    }
}

}
