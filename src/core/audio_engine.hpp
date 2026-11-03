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

#include "driver/mac_audio_driver_host.hpp" // also provides the bounded input queue

namespace Hirari::Core::BridgeFFI {

#if defined(__APPLE__)
using AudioDriverHost = ::Hirari::Core::Driver::MacAudioDriverHost;
#else
// Non-macOS builds currently have no native audio-device implementation.
// Keep the bridge constructible for offline/UI use, but never report a fake
// running device or fabricate callback/peak data.
class UnavailableAudioDriver {
public:
    bool start() noexcept { m_error = "native audio backend unavailable"; return false; }
    void stop() noexcept { m_running = false; }
    bool is_running() const noexcept { return false; }
    bool isSilentFallback() const noexcept { return true; }
    void try_reconnect() const noexcept { m_error = "native audio backend unavailable"; }
    using ProcessCallback = void (*)(const float* const*, uint32_t,
                                     float* const*, uint32_t, uint32_t,
                                     void*) noexcept;
    void set_process_callback(ProcessCallback callback, void* context) noexcept {
        m_callback = callback;
        m_context = context;
    }
    // The fallback has no hardware callback, but offline/test callers still
    // need the exact same callback boundary as JACK/CoreAudio. This explicit
    // dispatcher never claims a running device; it only invokes a callback
    // when the caller supplies a bounded output block.
    bool dispatch_process_callback(float* left, float* right, uint32_t frames) const noexcept {
        if (!m_callback || !left || !right || frames == 0) return false;
        float* outputs[2] = {left, right};
        m_callback(nullptr, 0, outputs, 2, frames, m_context);
        return true;
    }
    using InputBlockInfo = ::Hirari::Core::Driver::AudioInputBlockInfo;
    bool poll_input_block(float* const*, uint32_t, uint32_t, InputBlockInfo&, uint64_t& dropped) noexcept {
        dropped = 0;
        return false;
    }
    void capture_input(const float* const*, uint32_t, uint32_t) noexcept {}
    float output_peak() const noexcept { return 0.0f; }
    uint64_t callback_count() const noexcept { return 0; }
    uint64_t dropped_input_blocks() const noexcept { return 0; }
    void discard_pending_input_blocks() noexcept {}
    uint32_t input_channel_count() const noexcept { return 0; }
    uint32_t output_channel_count() const noexcept { return 0; }
    const char* last_error() const noexcept { return m_error.c_str(); }
    const char* status() const noexcept { return "unavailable"; }
private:
    mutable std::string m_error = "native audio backend unavailable";
    bool m_running = false;
    ProcessCallback m_callback = nullptr;
    void* m_context = nullptr;
};

using AudioDriverHost = UnavailableAudioDriver;
#endif

/**
 * @class AudioEngine
 * @brief Industrial-grade Audio Engine wrapper for Hirari Studio Pro.
 */
class AudioEngine {
#include "audio_engine_public_part_1.inc"
#include "audio_engine_public_part_2.inc"
#include "audio_engine_private.inc"

} // namespace Hirari::Core::BridgeFFI
