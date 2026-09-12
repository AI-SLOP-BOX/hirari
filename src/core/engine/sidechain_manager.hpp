#pragma once
#include <array>
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <memory>
#include <mutex>
#include <thread>
#include <vector>

namespace Aura::Core::Engine {

enum class SidechainTapPoint { PreFX, PostFX, PostFader };

// Kept for source compatibility with the original control-plane API.
struct SidechainSource {
    uint32_t trackId = 0;
    float level = 1.0f;
};

struct SidechainLink {
    const float* sourceBufferL = nullptr;
    const float* sourceBufferR = nullptr;
    uint32_t sourceTrackId = 0;
    float level = 1.0f;
    SidechainTapPoint tapPoint = SidechainTapPoint::PostFX;
    // When non-zero, registerLink copies the source block into the manager's
    // owned fixed-address buffers. This is the safe publication path.
    uint32_t sourceFrames = 0;
    uint32_t sourceSampleRate = 0;
    uint64_t sourceGeneration = 0;
};

/**
 * @class SidechainManager
 * @brief Industrial Sidechain Orchestration Engine.
 * HONEST FIX: Implemented O(1) direct buffer access and flexible tap points.
 */
class SidechainManager {
#include "sidechain_manager_public_part_1.inc"
#include "sidechain_manager_private_part_1.inc"
#include "sidechain_manager_public_part_2.inc"
#include "sidechain_manager_private_part_2.inc"

} // namespace Aura::Core::Engine
