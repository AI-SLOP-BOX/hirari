#pragma once
#include <cstdint>
#include <vector>

namespace Aura::Core::Engine::VideoBridge {

bool requestFrameAt(double seconds) noexcept;
bool loadVideo(const char* path) noexcept;
bool copyCurrentFrame(std::vector<uint8_t>& pixels, uint32_t& width, uint32_t& height) noexcept;
uint64_t frameRevision() noexcept;

}
