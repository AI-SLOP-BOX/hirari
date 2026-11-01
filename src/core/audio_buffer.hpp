#pragma once

#include <algorithm>
#include <memory>
#include <new>
#include <cmath>
#include "status_queue.hpp"
#include "rust_ffi.hpp"
#if !defined(_WIN32)
#include <sys/mman.h>
#endif

namespace Hirari::Core {

/**
 * @brief AudioBuffer: The 'Blood' of the DAW.
 * The Rust audio storage owns the sample block, the channel view and the
 * buffer dimensions; this class is only the typed C++ handle for them.
 */
class AudioBuffer {
public:
    static constexpr uint32_t kMaxFastPathChannels = 16;

    AudioBuffer() : m_storage(hirari_audio_buffer_storage_create()) {}

    AudioBuffer(uint32_t channels, uint32_t samples) : AudioBuffer() {
        resize(channels, samples);
    }

    ~AudioBuffer() { hirari_audio_buffer_storage_destroy(m_storage); }

    AudioBuffer(AudioBuffer&& other) noexcept : AudioBuffer() { *this = std::move(other); }
    AudioBuffer& operator=(AudioBuffer&& other) noexcept {
        if (this != &other) {
            hirari_audio_buffer_storage_destroy(m_storage);
            m_storage = other.m_storage;
            other.m_storage = hirari_audio_buffer_storage_create();
        }
        return *this;
    }

    AudioBuffer(const AudioBuffer&) = delete;
    AudioBuffer& operator=(const AudioBuffer&) = delete;

    bool reserve(uint32_t channels, uint32_t samples) {
        const uint8_t result =
            hirari_audio_buffer_storage_reserve_buffer(m_storage, channels, samples);
        if (result == 1 || result == 5) {
            pushEngineStatus(StatusSeverity::Critical,
                result == 1 ? "AUDIO_BUFFER_RESIZE_ATTEMPTED"
                            : "AUDIO_BUFFER_POINTER_RESIZE_ATTEMPTED");
            return false;
        }
        if (result == 2) throw std::bad_array_new_length();
        if (result != 0) throw std::bad_alloc();
        return true;
    }

    static void setRTThread(bool isRT) noexcept { hirari_audio_buffer_set_realtime_guard(isRT); }

    static uint64_t realtimeResizeAttempts() noexcept {
        return hirari_audio_buffer_realtime_resize_attempts();
    }

    static size_t lastRealtimeRequestedCapacity() noexcept {
        return hirari_audio_buffer_last_realtime_requested_capacity();
    }

    static void resetRealtimeResizeStats() noexcept {
        hirari_audio_buffer_reset_realtime_resize_stats();
    }

    bool resize(uint32_t channels, uint32_t samples) {
        if (!reserve(channels, samples)) return false;
        return hirari_audio_buffer_storage_resize(m_storage, channels, samples);
    }

    bool setSize(uint32_t channels, uint32_t samples) {
        return resize(channels, samples);
    }

    void wrapChannels(float** channels, uint32_t numChannels, uint32_t numSamples) {
        (void)hirari_audio_buffer_storage_wrap_channels(
            m_storage, channels, numChannels, numSamples);
    }

    float* getWritePointer(uint32_t channel) {
        return hirari_audio_buffer_storage_write_pointer(m_storage, channel);
    }

    const float* getReadPointer(uint32_t channel) const {
        return hirari_audio_buffer_storage_read_pointer(m_storage, channel, 0);
    }

    const float* getReadPointer(uint32_t channel, uint32_t offset) const {
        return hirari_audio_buffer_storage_read_pointer(m_storage, channel, offset);
    }

    float* getWritePointer(uint32_t channel, uint32_t offset) {
        return hirari_audio_buffer_storage_write_pointer_at(m_storage, channel, offset);
    }

    void clear() { hirari_audio_buffer_storage_clear(m_storage, 0, getNumSamples()); }

    void clear(uint32_t samples) { hirari_audio_buffer_storage_clear(m_storage, 0, samples); }

    void clear(uint32_t offset, uint32_t samples) {
        hirari_audio_buffer_storage_clear(m_storage, offset, samples);
    }

    // Boundary sanitizer for third-party DSP.  A plugin is not allowed to
    // poison the rest of the graph with NaN/Inf; the scan stays allocation free
    // so it is safe on the audio thread.
    uint32_t sanitizeNonFinite() noexcept {
        return hirari_audio_buffer_storage_sanitize(m_storage);
    }

    void addFrom(const AudioBuffer& other, uint32_t numSamples) {
        hirari_audio_buffer_storage_add_from(m_storage, other.m_storage, numSamples);
    }

    void addFrom(const float* srcL, const float* srcR, uint32_t numSamples) {
        hirari_audio_buffer_storage_add_channels_from(m_storage, srcL, srcR, numSamples);
    }

    void applyGain(float gain) {
        hirari_audio_buffer_storage_apply_gain(m_storage, gain);
    }

    uint32_t getNumChannels() const { return hirari_audio_buffer_storage_channels(m_storage); }
    uint32_t getNumSamples() const { return hirari_audio_buffer_storage_samples(m_storage); }

    const float* const* getArrayOfReadPointers() const {
        return reinterpret_cast<const float* const*>(
            hirari_audio_buffer_storage_array_read(m_storage));
    }

    float** getArrayOfWritePointers() {
        return hirari_audio_buffer_storage_array_write(m_storage);
    }

private:
    void* m_storage = nullptr;
};

} // namespace Hirari::Core
