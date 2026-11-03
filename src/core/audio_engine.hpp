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
// running device or fabricate callback/peak data. The telemetry the UI polls is
// answered from Rust (`audio_driver_view`), so only the callback boundary and
// the lifecycle calls the engine's own control flow needs live here.
class UnavailableAudioDriver {
public:
    bool start() noexcept { return false; }
    void stop() noexcept {}
    bool is_running() const noexcept { return false; }
    void try_reconnect() noexcept {}
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
    void discard_pending_input_blocks() noexcept {}
    uint32_t input_channel_count() const noexcept { return 0; }
private:
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
