#pragma once
#include <vector>
#include <algorithm>
#include <cmath>
#include <cstring>
#include <cstdlib>
#include <string>
#include <memory>
#include <new>
#include <mutex>
#include <thread>
#include <unordered_map>
#include <unordered_set>
#include "rust/cxx.h"
#include "hirari_unified_engine.hpp"
#include "bridge_types.hpp"

// The device host is Rust (`audio_driver_host.rs`). It owns the lifecycle
// defaults, the bounded input queue and the platform split the two deleted C++
// adapter classes carried, so the engine holds one opaque handle and the same
// calls answer off macOS as a silent host that never claims a running device.
using AudioDriverProcessCallback = void (*)(const float* const*, uint32_t,
                                            float* const*, uint32_t, uint32_t,
                                            void*) noexcept;

extern "C" {
void* hirari_audio_driver_host_create(AudioDriverProcessCallback process, void* context);
void hirari_audio_driver_host_destroy(void* host);
// Claiming the device is the Rust host's creation policy: it declines for an
// offline or isolated session and publishes the failure itself, so the engine
// has no message formatting or notification code left here.
bool hirari_audio_driver_host_start_at_creation(void* host, bool requested);
void hirari_audio_driver_host_stop(void* host);
bool hirari_audio_driver_host_is_running(void* host);
void hirari_audio_driver_host_try_reconnect(void* host);
bool hirari_audio_driver_host_dispatch_process(void* host, float* left, float* right,
                                               uint32_t frames);
}

namespace Hirari::Core::BridgeFFI {

/**
 * @class AudioEngine
 * @brief Industrial-grade Audio Engine wrapper for Hirari Studio Pro.
 */
class AudioEngine {
#include "audio_engine_public_part_1.inc"
#include "audio_engine_public_part_2.inc"
#include "audio_engine_private.inc"

} // namespace Hirari::Core::BridgeFFI
