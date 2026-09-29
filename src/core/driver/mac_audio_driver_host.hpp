#pragma once
#include <algorithm>
#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <mutex>
#include "../rust_ffi.hpp"
#if defined(__APPLE__)
#include <CoreAudio/CoreAudio.h>
#include "mac_audio_driver.hpp"
#endif

namespace Hirari::Core::Driver {

/**
 * ABI adapter to the Rust-owned fixed-capacity SPSC input queue.
 */
class MacAudioInputBlockQueue final {
public:
    static constexpr uint32_t kCapacity = 8;
    static constexpr uint32_t kMaxChannels = 32;
    static constexpr uint32_t kMaxFrames = 8192;

    struct BlockInfo {
        uint32_t channelCount = 0;
        uint32_t frameCount = 0;
    };

    MacAudioInputBlockQueue() : m_queue(hirari_audio_input_queue_create()) {}
    ~MacAudioInputBlockQueue() { hirari_audio_input_queue_free(m_queue); }
    MacAudioInputBlockQueue(const MacAudioInputBlockQueue&) = delete;
    MacAudioInputBlockQueue& operator=(const MacAudioInputBlockQueue&) = delete;

    bool push_planar(const float* const* channels,
                     uint32_t channelCount,
                     uint32_t frameCount) noexcept {
        return hirari_audio_input_queue_push(m_queue, channels, channelCount, frameCount);
    }

    // droppedBlocks is the number accumulated since the previous poll. It is
    // reported even when no complete block is currently available.
    bool poll(float* const* destination,
              uint32_t destinationChannelCapacity,
              uint32_t destinationFrameCapacity,
              BlockInfo& info,
              uint64_t& droppedBlocks) noexcept {
        return hirari_audio_input_queue_poll(
            m_queue, destination, destinationChannelCapacity, destinationFrameCapacity,
            &info.channelCount, &info.frameCount, &droppedBlocks);
    }

    uint64_t dropped_blocks() const noexcept {
        return hirari_audio_input_queue_dropped(m_queue);
    }

    // Discard input accumulated before a new recording starts. The callback
    // remains lock-free; publish the producer's current head as the consumer
    // head, then clear losses that occurred while the queue was idle.
    void discard_pending() noexcept {
        hirari_audio_input_queue_discard(m_queue);
    }

    // Called only after the driver has stopped and its callbacks are quiescent.
    void reset() noexcept {
        hirari_audio_input_queue_reset(m_queue);
    }

private:
    void* m_queue = nullptr;
};

#if defined(__APPLE__)

class MacAudioDriverHost {
public:
    using InputCaptureSink = MacAudioDriver::InputCaptureSink;
    using InputBlockInfo = MacAudioInputBlockQueue::BlockInfo;
    using ProcessCallback = void (*)(const float* const*, uint32_t,
                                     float* const*, uint32_t, uint32_t,
                                     void*) noexcept;

    MacAudioDriverHost();
    ~MacAudioDriverHost();
    // Returns true only after the device callback is running.  Callers must
    // not infer readiness from construction or from status alone.
    bool start();
    void stop();
    bool is_running() const;
    double sample_rate() const noexcept;
    uint32_t buffer_size() const noexcept;
    uint32_t input_channel_count() const;
    uint32_t output_channel_count() const noexcept;
    const char* status() const noexcept;
    int32_t last_error_code() const noexcept;
    void try_reconnect();
    bool reconfigure(double sampleRate, uint32_t bufferSize);
    std::string list_devices_json() const;
    std::string selected_device_uid() const;
    bool select_device(uint32_t deviceId, double sampleRate, uint32_t bufferSize);
    void set_process_callback(ProcessCallback callback, void* context) noexcept;
    float output_peak() const;
    uint64_t callback_count() const;

    // Non-realtime consumer API. The destination must provide at least
    // kMaxChannels pointers and kMaxFrames samples per channel for all blocks
    // accepted by the queue. droppedBlocks is reset/reported on every poll.
    bool poll_input_block(float* const* destination,
                          uint32_t destinationChannelCapacity,
                          uint32_t destinationFrameCapacity,
                          InputBlockInfo& info,
                          uint64_t& droppedBlocks) noexcept;
    void discard_pending_input_blocks() noexcept;
    void capture_input(const float* const* channels,
                       uint32_t channelCount,
                       uint32_t frameCount) noexcept;
    uint64_t dropped_input_blocks() const noexcept;

    // The sink must outlive unregister_input_capture_sink(). Its callback is
    // invoked on the CoreAudio realtime thread and must obey the restrictions
    // documented by MacAudioDriver::InputCaptureSink.
    bool register_input_capture_sink(const InputCaptureSink* sink) noexcept;
    void unregister_input_capture_sink() noexcept;
private:
#if defined(__APPLE__)
    static OSStatus defaultDeviceListener(AudioObjectID, UInt32,
                                          const AudioObjectPropertyAddress[], void*);
#endif
    bool start_locked();
    bool start_locked(double sampleRate, uint32_t bufferSize);
    void stop_locked();

    struct Impl;
    std::unique_ptr<Impl> m_impl;
};

#endif // defined(__APPLE__)

} // namespace Hirari::Core::Driver
