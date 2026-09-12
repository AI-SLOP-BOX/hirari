#pragma once
#include <vector>
#include <algorithm>
#include <cmath>
#include <cstring>
#include <cstdlib>
#include <string>
#include <memory>
#include <mutex>
#include <thread>
#include <unordered_map>
#include <unordered_set>
#include "rust/cxx.h"
#include "aura_unified_engine.hpp"
#include "plugins/native_editor_host.hpp"
#include "dsp/vocal/psola_pitch_shifter.hpp"
#include "dsp/analysis/spectral_processor.hpp"
#include "bridge_types.hpp"
#include "../io/wav_loader_utils.hpp"

#include "driver/mac_audio_driver_host.hpp" // also provides the bounded input queue
#if defined(__APPLE__)
#include "driver/mac_midi_device_host.hpp"
#elif defined(AURA_ENABLE_JACK)
#include "external/jack_bridge_deep.hpp"
#endif

namespace Aura::Core::BridgeFFI {

#if defined(__APPLE__)
using AudioDriverHost = ::Aura::Core::Driver::MacAudioDriverHost;
#elif defined(AURA_ENABLE_JACK)
// JACK is opt-in at compile time.  When enabled, this adapter is the driver
// owned by the same AudioEngine session as the macOS CoreAudio host; the JACK
// realtime callback therefore enters the production AuraUnifiedEngine graph
// instead of a parallel test-only graph.
class JackAudioDriverHost final {
public:
    using ProcessCallback = ::Aura::Core::External::JackBridgeDeep::ProcessCallback;

    bool start() {
        auto& jack = ::Aura::Core::External::JackBridgeDeep::getInstance();
        if (!jack.tryInitialize("Aura")) {
            m_status = "start-failed";
            m_error = jack.lastError();
            return false;
        }
        jack.setProcessCallback(m_callback, m_context);
        m_running = true;
        m_status = "running";
        m_error.clear();
        return true;
    }

    void stop() noexcept {
        m_running = false;
        auto& jack = ::Aura::Core::External::JackBridgeDeep::getInstance();
        jack.setProcessCallback(nullptr, nullptr);
        jack.shutdown();
        m_status = "stopped";
    }

    bool is_running() const noexcept { return m_running &&
        ::Aura::Core::External::JackBridgeDeep::getInstance().isRunning(); }
    double sample_rate() const noexcept {
        return ::Aura::Core::External::JackBridgeDeep::getInstance().sampleRate();
    }
    uint32_t buffer_size() const noexcept {
        return ::Aura::Core::External::JackBridgeDeep::getInstance().bufferSize();
    }
    bool isSilentFallback() const noexcept { return false; }
    void try_reconnect() { if (!is_running()) (void)start(); }

    bool reconfigure(double sampleRate, uint32_t bufferSize) {
        if (!std::isfinite(sampleRate) || sampleRate < 8000.0 ||
            sampleRate > 384000.0 || bufferSize == 0) {
            m_error = "invalid JACK configuration";
            return false;
        }
        // JACK negotiates these values with the server.  A requested format
        // is accepted only when the server reports the same format after the
        // restart; this avoids claiming a graph/device match that is false.
        stop();
        if (!start()) return false;
        const auto& jack = ::Aura::Core::External::JackBridgeDeep::getInstance();
        if (std::abs(jack.sampleRate() - sampleRate) > 0.5 ||
            jack.bufferSize() != bufferSize) {
            m_error = "JACK server rejected the requested sample rate or buffer size";
            stop();
            m_status = "start-failed";
            return false;
        }
        return true;
    }

    const char* status() const noexcept { return m_status.c_str(); }
    int32_t last_error_code() const noexcept { return m_error.empty() ? 0 : 1; }
    const char* last_error() const noexcept { return m_error.c_str(); }
    float output_peak() const noexcept { return m_outputPeak.load(std::memory_order_acquire); }
    uint64_t callback_count() const noexcept { return m_callbackCount.load(std::memory_order_acquire); }
    uint64_t dropped_input_blocks() const noexcept { return m_inputQueue.dropped_blocks(); }
    bool poll_input_block(float* const* destination,
                          uint32_t destinationChannelCapacity,
                          uint32_t destinationFrameCapacity,
                          ::Aura::Core::Driver::MacAudioInputBlockQueue::BlockInfo& info,
                          uint64_t& droppedBlocks) noexcept {
        return m_inputQueue.poll(destination, destinationChannelCapacity,
                                 destinationFrameCapacity, info, droppedBlocks);
    }
    void capture_input(const float* const* channels, uint32_t channelCount,
                       uint32_t frameCount) noexcept {
        (void)m_inputQueue.push_planar(channels, channelCount, frameCount);
    }
    const char* list_devices_json() const noexcept {
        return "[{\"id\":0,\"name\":\"JACK server\",\"api\":\"JACK\"}]";
    }
    bool select_device(uint32_t deviceId, double sampleRate, uint32_t bufferSize) {
        return deviceId == 0 && reconfigure(sampleRate, bufferSize);
    }
    void set_process_callback(ProcessCallback callback, void* context) noexcept {
        m_callback = callback;
        m_context = context;
        if (is_running()) {
            ::Aura::Core::External::JackBridgeDeep::getInstance().setProcessCallback(callback, context);
        }
    }

    void record_callback(uint32_t frames, float peak) noexcept {
        m_callbackCount.fetch_add(1, std::memory_order_relaxed);
        m_outputPeak.store(peak, std::memory_order_relaxed);
        (void)frames;
    }

private:
    ProcessCallback m_callback = nullptr;
    void* m_context = nullptr;
    std::atomic<bool> m_running{false};
    std::atomic<uint64_t> m_callbackCount{0};
    std::atomic<float> m_outputPeak{0.0f};
    ::Aura::Core::Driver::MacAudioInputBlockQueue m_inputQueue;
    std::string m_status = "stopped";
    std::string m_error;
};
using AudioDriverHost = JackAudioDriverHost;
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
    using ProcessCallback = void (*)(const float* const*, float* const*, uint32_t, void*) noexcept;
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
        m_callback(nullptr, outputs, frames, m_context);
        return true;
    }
    using InputBlockInfo = ::Aura::Core::Driver::MacAudioInputBlockQueue::BlockInfo;
    bool poll_input_block(float* const*, uint32_t, uint32_t, InputBlockInfo&, uint64_t& dropped) noexcept {
        dropped = 0;
        return false;
    }
    void capture_input(const float* const*, uint32_t, uint32_t) noexcept {}
    float output_peak() const noexcept { return 0.0f; }
    uint64_t callback_count() const noexcept { return 0; }
    uint64_t dropped_input_blocks() const noexcept { return 0; }
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

struct BridgeScoreGlyph {
    uint32_t type;
    float x, y;
};

/**
 * @class AudioEngine
 * @brief Industrial-grade Audio Engine wrapper for Aura Studio Pro.
 */
class AudioEngine {
#include "audio_engine_public_part_1.inc"
#include "audio_engine_public_part_2.inc"
#include "audio_engine_private.inc"

} // namespace Aura::Core::BridgeFFI
