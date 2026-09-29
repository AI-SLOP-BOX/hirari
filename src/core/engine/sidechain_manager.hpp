#pragma once
#include <bit>
#include <atomic>
#include <cstdint>
#include <mutex>
#include <utility>
#include <vector>

extern "C" void* hirari_sidechain_publication_gate_create();
extern "C" void hirari_sidechain_publication_gate_destroy(void* gate);
extern "C" bool hirari_sidechain_publication_gate_enter_reader(const void* gate);
extern "C" void hirari_sidechain_publication_gate_leave_reader(const void* gate);
extern "C" void hirari_sidechain_publication_gate_begin_control(const void* gate);
extern "C" void hirari_sidechain_publication_gate_end_control(const void* gate);
extern "C" bool hirari_sidechain_publication_gate_try_begin_audio(const void* gate);
extern "C" void hirari_sidechain_publication_gate_end_audio(const void* gate);
extern "C" bool hirari_sidechain_register_link(const void*, uint32_t, uint32_t, uint32_t, const float*, const float*, uint32_t, uint32_t, uint64_t, float, uint8_t);
extern "C" bool hirari_sidechain_register_silent_link(const void*, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t, uint64_t, float, uint8_t);
extern "C" bool hirari_sidechain_remove_link(const void*, uint32_t, uint32_t);
extern "C" bool hirari_sidechain_has_link(const void*, uint32_t, uint32_t, uint32_t);
extern "C" float hirari_sidechain_level(const void*, uint32_t, uint32_t);
extern "C" void hirari_sidechain_remove_track(const void*, uint32_t);
extern "C" void hirari_sidechain_reset(const void*);
extern "C" uint32_t hirari_sidechain_copy_link(const void*, uint32_t, uint32_t, float*, float*, uint32_t, uint64_t*);
extern "C" bool hirari_sidechain_refresh_source(const void*, uint32_t, const float*, const float*, uint32_t, uint32_t, uint64_t);
extern "C" void hirari_sidechain_publish_source(const void*, uint32_t, const float*, const float*, const float*, const float*, const float*, const float*, uint32_t, uint32_t, uint64_t);
using HirariSidechainCaptureCallback = bool (*)(void*, uint32_t, uint32_t, uint32_t, float, uint8_t, uint32_t, uint64_t, const float*, const float*, uint32_t);
extern "C" bool hirari_sidechain_capture_track(const void*, uint32_t, void*, HirariSidechainCaptureCallback);
namespace Hirari::Core::Engine {

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
#include "sidechain_manager_public_part_2.inc"
#include "sidechain_manager_private_part_2.inc"

} // namespace Hirari::Core::Engine
