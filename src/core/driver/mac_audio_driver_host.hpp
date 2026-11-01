#pragma once
#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <string>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Driver {

// Fixed capacities of the Rust-owned SPSC input queue. Callers size their
// scratch buffers from these, so they live with the block descriptor instead
// of a C++ wrapper class.
constexpr uint32_t kAudioInputQueueChannels = 32;
constexpr uint32_t kAudioInputQueueFrames = 8192;

struct AudioInputBlockInfo {
    uint32_t channelCount = 0;
    uint32_t frameCount = 0;
};

} // namespace Hirari::Core::Driver

#if defined(__APPLE__)

struct AudioTimeStamp;

using HirariMacProcessCallback = void (*)(const float* const*, uint32_t,
                                          float* const*, uint32_t, uint32_t,
                                          void*) noexcept;
using HirariMacLogCallback = void (*)(void*, uint32_t, const char*);

extern "C" {
void* hirari_mac_audio_host_create(HirariMacProcessCallback process,
                                   void* processContext,
                                   HirariMacLogCallback log,
                                   void* logContext);
void hirari_mac_audio_host_destroy(void* host);
bool hirari_mac_audio_host_start(void* host);
void hirari_mac_audio_host_stop(void* host);
bool hirari_mac_audio_host_is_running(void* host);
double hirari_mac_audio_host_sample_rate(void* host);
uint32_t hirari_mac_audio_host_buffer_size(void* host);
uint32_t hirari_mac_audio_host_input_channels(void* host);
uint32_t hirari_mac_audio_host_output_channels(void* host);
const char* hirari_mac_audio_host_status(void* host);
int32_t hirari_mac_audio_host_last_error_code(void* host);
void hirari_mac_audio_host_try_reconnect(void* host);
bool hirari_mac_audio_host_reconfigure(void* host, double sampleRate, uint32_t bufferSize);
bool hirari_mac_audio_host_select_device(void* host, uint32_t device,
                                         double sampleRate, uint32_t bufferSize);
char* hirari_mac_audio_host_list_devices_json(void* host);
char* hirari_mac_audio_host_selected_device_uid(void* host);
void hirari_mac_audio_string_free(char* value);
void hirari_mac_audio_host_set_process_callback(void* host,
                                                HirariMacProcessCallback process,
                                                void* context);
float hirari_mac_audio_host_output_peak(void* host);
uint64_t hirari_mac_audio_host_callback_count(void* host);
bool hirari_mac_audio_host_poll_input_block(void* host, float* const* destination,
                                            uint32_t destinationChannels,
                                            uint32_t destinationFrames,
                                            Hirari::Core::Driver::AudioInputBlockInfo* info,
                                            uint64_t* dropped);
void hirari_mac_audio_host_discard_pending_input(void* host);
void hirari_mac_audio_host_capture_input(void* host, const float* const* channels,
                                         uint32_t channelCount, uint32_t frameCount);
uint64_t hirari_mac_audio_host_dropped_input_blocks(void* host);
}

namespace Hirari::Core::Driver {

/**
 * Engine-facing macOS audio host contract.
 *
 * The implementation is Rust (`mac_audio_driver.rs`). This class only marshals
 * the engine's callback boundary and the public status/device surface across
 * the C ABI. It deliberately keeps the pre-port method names and status text.
 */
class MacAudioDriverHost {
public:
    using ProcessCallback = void (*)(const float* const*, uint32_t,
                                     float* const*, uint32_t, uint32_t,
                                     void*) noexcept;

    MacAudioDriverHost()
        : m_handle(hirari_mac_audio_host_create(nullptr, nullptr,
                                                &log_trampoline, nullptr)) {}
    ~MacAudioDriverHost() { hirari_mac_audio_host_destroy(m_handle); }

    MacAudioDriverHost(const MacAudioDriverHost&) = delete;
    MacAudioDriverHost& operator=(const MacAudioDriverHost&) = delete;

    // Returns true only after the device callback is running. Callers must
    // not infer readiness from construction or from status alone.
    bool start() { return m_handle != nullptr && hirari_mac_audio_host_start(m_handle); }
    void stop() {
        if (m_handle) hirari_mac_audio_host_stop(m_handle);
    }
    bool is_running() const {
        return m_handle != nullptr && hirari_mac_audio_host_is_running(m_handle);
    }
    double sample_rate() const noexcept {
        return m_handle ? hirari_mac_audio_host_sample_rate(m_handle) : 44100.0;
    }
    uint32_t buffer_size() const noexcept {
        return m_handle ? hirari_mac_audio_host_buffer_size(m_handle) : 512u;
    }
    uint32_t input_channel_count() const {
        return m_handle ? hirari_mac_audio_host_input_channels(m_handle) : 0u;
    }
    uint32_t output_channel_count() const noexcept {
        return m_handle ? hirari_mac_audio_host_output_channels(m_handle) : 0u;
    }
    const char* status() const noexcept {
        const char* value = m_handle ? hirari_mac_audio_host_status(m_handle) : nullptr;
        return value ? value : "unavailable";
    }
    int32_t last_error_code() const noexcept {
        return m_handle ? hirari_mac_audio_host_last_error_code(m_handle) : 0;
    }
    void try_reconnect() {
        if (m_handle) hirari_mac_audio_host_try_reconnect(m_handle);
    }
    bool reconfigure(double sampleRate, uint32_t bufferSize) {
        return m_handle != nullptr &&
               hirari_mac_audio_host_reconfigure(m_handle, sampleRate, bufferSize);
    }
    std::string list_devices_json() const {
        return take_string(m_handle ? hirari_mac_audio_host_list_devices_json(m_handle)
                                    : nullptr);
    }
    std::string selected_device_uid() const {
        return take_string(m_handle ? hirari_mac_audio_host_selected_device_uid(m_handle)
                                    : nullptr);
    }
    bool select_device(uint32_t deviceId, double sampleRate, uint32_t bufferSize) {
        return m_handle != nullptr &&
               hirari_mac_audio_host_select_device(m_handle, deviceId, sampleRate,
                                                   bufferSize);
    }
    void set_process_callback(ProcessCallback callback, void* context) noexcept {
        if (m_handle)
            hirari_mac_audio_host_set_process_callback(m_handle, callback, context);
    }
    float output_peak() const {
        return m_handle ? hirari_mac_audio_host_output_peak(m_handle) : 0.0f;
    }
    uint64_t callback_count() const {
        return m_handle ? hirari_mac_audio_host_callback_count(m_handle) : 0;
    }

    // Non-realtime consumer API. The destination must provide at least
    // kAudioInputQueueChannels pointers and kAudioInputQueueFrames samples per
    // channel for all blocks accepted by the queue.
    bool poll_input_block(float* const* destination,
                          uint32_t destinationChannelCapacity,
                          uint32_t destinationFrameCapacity,
                          AudioInputBlockInfo& info,
                          uint64_t& droppedBlocks) noexcept {
        if (!m_handle) {
            info = {};
            droppedBlocks = 0;
            return false;
        }
        return hirari_mac_audio_host_poll_input_block(
            m_handle, destination, destinationChannelCapacity, destinationFrameCapacity,
            &info, &droppedBlocks);
    }
    void discard_pending_input_blocks() noexcept {
        if (m_handle) hirari_mac_audio_host_discard_pending_input(m_handle);
    }
    void capture_input(const float* const* channels, uint32_t channelCount,
                       uint32_t frameCount) noexcept {
        if (m_handle)
            hirari_mac_audio_host_capture_input(m_handle, channels, channelCount,
                                                frameCount);
    }
    uint64_t dropped_input_blocks() const noexcept {
        return m_handle ? hirari_mac_audio_host_dropped_input_blocks(m_handle) : 0;
    }

private:
    static void log_trampoline(void*, uint32_t level, const char* message) noexcept {
        hirari_log_post(level, 0xA001, message);
    }

    static std::string take_string(char* value) {
        std::string result = value ? std::string(value) : std::string{};
        hirari_mac_audio_string_free(value);
        return result;
    }

    void* m_handle = nullptr;
};

} // namespace Hirari::Core::Driver

#endif // defined(__APPLE__)
