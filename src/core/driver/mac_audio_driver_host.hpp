#pragma once
#include <algorithm>
#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <mutex>
#if defined(__APPLE__)
#include <CoreAudio/CoreAudio.h>
#include "mac_audio_driver.hpp"
#endif

namespace Aura::Core::Driver {

/**
 * Fixed-capacity SPSC queue for planar CoreAudio input blocks.
 *
 * The producer is the CoreAudio callback and the consumer is a non-realtime
 * polling thread. The queue owns all sample storage; push_planar() never
 * allocates or takes a lock.
 */
class MacAudioInputBlockQueue final {
public:
    static constexpr uint32_t kCapacity = 8;
    static constexpr uint32_t kMaxChannels = 2;
    static constexpr uint32_t kMaxFrames = 4096;

    struct BlockInfo {
        uint32_t channelCount = 0;
        uint32_t frameCount = 0;
    };

    bool push_planar(const float* const* channels,
                     uint32_t channelCount,
                     uint32_t frameCount) noexcept {
        const uint64_t write = m_writeIndex.load(std::memory_order_relaxed);
        const uint64_t read = m_readIndex.load(std::memory_order_acquire);
        if (!channels || channelCount == 0 || channelCount > kMaxChannels ||
            frameCount == 0 || frameCount > kMaxFrames || write - read >= kCapacity) {
            m_droppedBlocks.fetch_add(1, std::memory_order_relaxed);
            return false;
        }

        Slot& slot = m_slots[write & (kCapacity - 1)];
        for (uint32_t channel = 0; channel < channelCount; ++channel) {
            if (!channels[channel]) {
                m_droppedBlocks.fetch_add(1, std::memory_order_relaxed);
                return false;
            }
            std::copy_n(channels[channel], frameCount,
                        slot.samples.data() + static_cast<std::size_t>(channel) * kMaxFrames);
        }
        slot.info = {channelCount, frameCount};
        m_writeIndex.store(write + 1, std::memory_order_release);
        return true;
    }

    // droppedBlocks is the number accumulated since the previous poll. It is
    // reported even when no complete block is currently available.
    bool poll(float* const* destination,
              uint32_t destinationChannelCapacity,
              uint32_t destinationFrameCapacity,
              BlockInfo& info,
              uint64_t& droppedBlocks) noexcept {
        droppedBlocks = m_droppedBlocks.exchange(0, std::memory_order_acq_rel);
        const uint64_t read = m_readIndex.load(std::memory_order_relaxed);
        const uint64_t write = m_writeIndex.load(std::memory_order_acquire);
        if (read == write) return false;

        const Slot& slot = m_slots[read & (kCapacity - 1)];
        info = slot.info;
        if (!destination || info.channelCount > destinationChannelCapacity ||
            info.frameCount > destinationFrameCapacity) {
            return false;
        }
        for (uint32_t channel = 0; channel < info.channelCount; ++channel) {
            if (!destination[channel]) return false;
            std::copy_n(slot.samples.data() + static_cast<std::size_t>(channel) * kMaxFrames,
                        info.frameCount, destination[channel]);
        }
        m_readIndex.store(read + 1, std::memory_order_release);
        return true;
    }

    uint64_t dropped_blocks() const noexcept {
        return m_droppedBlocks.load(std::memory_order_acquire);
    }

    // Called only after the driver has stopped and its callbacks are quiescent.
    void reset() noexcept {
        const uint64_t write = m_writeIndex.load(std::memory_order_relaxed);
        m_readIndex.store(write, std::memory_order_relaxed);
        m_droppedBlocks.store(0, std::memory_order_relaxed);
    }

private:
    struct Slot {
        std::array<float, static_cast<size_t>(kMaxChannels) * kMaxFrames> samples{};
        BlockInfo info{};
    };

    std::array<Slot, kCapacity> m_slots{};
    alignas(64) std::atomic<uint64_t> m_writeIndex{0};
    alignas(64) std::atomic<uint64_t> m_readIndex{0};
    std::atomic<uint64_t> m_droppedBlocks{0};
};

#if defined(__APPLE__)

class MacAudioDriverHost {
public:
    using InputCaptureSink = MacAudioDriver::InputCaptureSink;
    using InputBlockInfo = MacAudioInputBlockQueue::BlockInfo;

    MacAudioDriverHost();
    ~MacAudioDriverHost();
    // Returns true only after the device callback is running.  Callers must
    // not infer readiness from construction or from status alone.
    bool start();
    void stop();
    bool is_running() const;
    const char* status() const noexcept;
    int32_t last_error_code() const noexcept;
    void try_reconnect();
    bool reconfigure(double sampleRate, uint32_t bufferSize);
    std::string list_devices_json() const;
    bool select_device(uint32_t deviceId, double sampleRate, uint32_t bufferSize);
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

} // namespace Aura::Core::Driver
