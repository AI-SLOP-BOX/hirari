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
 * HONEST FIX: Unified the memory allocation into a single contiguous block.
 */
class AudioBuffer {
public:
    static constexpr uint32_t kMaxFastPathChannels = 16;

    AudioBuffer() : m_numChannels(0), m_numSamples(0), m_capacity(0),
                    m_isExternal(false), m_data(nullptr) {}
    
    AudioBuffer(uint32_t channels, uint32_t samples) : AudioBuffer() {
        resize(channels, samples);
    }

    ~AudioBuffer() { hirari_audio_buffer_storage_destroy(m_storage); }

    /**
     * @brief Pins the buffer in physical RAM to prevent kernel page-faults.
     */
    AudioBuffer(AudioBuffer&& other) noexcept : AudioBuffer() { *this = std::move(other); }
    AudioBuffer& operator=(AudioBuffer&& other) noexcept {
        if (this != &other) {
            hirari_audio_buffer_storage_destroy(m_storage);
            m_numChannels = other.m_numChannels;
            m_numSamples = other.m_numSamples;
            m_capacity = other.m_capacity;
            m_storage = other.m_storage;
            m_data = other.m_data;
            m_isExternal = other.m_isExternal;
            m_externalData = other.m_externalData;
            m_isDirty = other.m_isDirty;
            other.m_storage = nullptr;
            other.m_data = nullptr;
            other.m_externalData = nullptr;
            other.m_numChannels = 0;
            other.m_numSamples = 0;
            other.m_capacity = 0;
            other.m_isExternal = false;
            other.m_isDirty = true;
            updatePointers();
        }
        return *this;
    }

    AudioBuffer(const AudioBuffer&) = delete;
    AudioBuffer& operator=(const AudioBuffer&) = delete;

    bool reserve(uint32_t channels, uint32_t samples) {
        bool needsDataAllocation = false;
        const uint8_t result = hirari_audio_buffer_storage_reserve_buffer(
            &m_storage, channels, m_numChannels, samples, m_capacity,
            m_isExternal, &needsDataAllocation);
        if (result == 1) {
            pushEngineStatus(StatusSeverity::Critical,
                needsDataAllocation ? "AUDIO_BUFFER_RESIZE_ATTEMPTED"
                                    : "AUDIO_BUFFER_POINTER_RESIZE_ATTEMPTED");
            return false;
        }
        if (result == 2) throw std::bad_array_new_length();
        if (result == 3) throw std::bad_alloc();
        if (result == 4) {
            m_data = hirari_audio_buffer_storage_data(m_storage);
            m_capacity = hirari_audio_buffer_storage_capacity(m_storage);
            if (needsDataAllocation) m_isExternal = false;
            throw std::bad_alloc();
        }
        if (result != 0) throw std::bad_alloc();
        if (needsDataAllocation) {
            m_data = hirari_audio_buffer_storage_data(m_storage);
            m_capacity = hirari_audio_buffer_storage_capacity(m_storage);
            m_isExternal = false;
        }
        updatePointers();
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
        m_isExternal = false;
        m_externalData = nullptr;
        m_numChannels = channels;
        m_numSamples = samples;
        updatePointers();
        clear();
        return true;
    }

    bool setSize(uint32_t channels, uint32_t samples) {
        return resize(channels, samples);
    }

    void wrapChannels(float** channels, uint32_t numChannels, uint32_t numSamples) {
        if (hirari_audio_buffer_storage_wrap_channels(
                &m_storage, &m_numChannels, &m_numSamples, &m_capacity,
                &m_data, &m_isExternal, &m_externalData, channels, numChannels, numSamples)) {
            m_isDirty = true;
        }
    }

    float* getWritePointer(uint32_t channel) { 
        m_isDirty = true; 
        if (channel >= m_numChannels) return nullptr;
        if (m_isExternal) return m_externalData[channel];
        return m_data + (channel * m_numSamples);
    }
    
    const float* getReadPointer(uint32_t channel) const { 
        if (channel >= m_numChannels) return nullptr;
        if (m_isExternal) return m_externalData[channel];
        return m_data + (channel * m_numSamples);
    }
    const float* getReadPointer(uint32_t channel, uint32_t offset) const {
        const float* pointer = getReadPointer(channel);
        return pointer ? pointer + std::min(offset, m_numSamples) : nullptr;
    }
    float* getWritePointer(uint32_t channel, uint32_t offset) {
        float* pointer = getWritePointer(channel);
        return pointer ? pointer + std::min(offset, m_numSamples) : nullptr;
    }

    void clear() {
        clear(m_numSamples);
    }

    void clear(uint32_t samples) {
        clear(0, samples);
    }

    void clear(uint32_t offset, uint32_t samples) {
        if (offset >= m_numSamples || m_numChannels == 0) return;
        uint32_t n = std::min(samples, m_numSamples - offset);
        hirari_audio_buffer_clear(getArrayOfWritePointers(), m_numChannels, offset, n);
        m_isDirty = false;
    }

    // Boundary sanitizer for third-party DSP.  A plugin is not allowed to
    // poison the rest of the graph with NaN/Inf; keep the operation allocation
    // free so it is safe on the audio thread.
    uint32_t sanitizeNonFinite() noexcept {
        if (m_numChannels > 0) m_isDirty = true;
        return hirari_audio_buffer_sanitize_non_finite(
            getArrayOfWritePointers(), m_numChannels, m_numSamples);
    }

    void addFrom(const AudioBuffer& other, uint32_t numSamples) {
        uint32_t channels = std::min(m_numChannels, other.getNumChannels());
        uint32_t n = std::min(numSamples, std::min(m_numSamples, other.getNumSamples()));

        if (channels > 0) m_isDirty = true;
        hirari_audio_buffer_add_channels(
            getArrayOfWritePointers(), other.getArrayOfReadPointers(), channels, n);
    }

    void addFrom(const float* srcL, const float* srcR, uint32_t numSamples) {
        // Raw-pointer callers include device/plugin bridges.  A missing
        // channel must never turn a disconnect or malformed callback into a
        // null dereference on the realtime thread.
        if (srcL == nullptr || (m_numChannels > 1 && srcR == nullptr)) return;
        uint32_t n = std::min(numSamples, m_numSamples);
        if (m_numChannels < 1) return;

        float* dL = getWritePointer(0);
        float* dR = m_numChannels > 1 ? getWritePointer(1) : nullptr;
        float* destinations[] = {dL, dR};
        const float* sources[] = {srcL, srcR};
        hirari_audio_buffer_add_channels(destinations, sources, dR ? 2u : 1u, n);
    }

    void applyGain(float gain) {
        // Gain values can arrive from automation or an external control
        // surface. Reject non-finite values at the buffer boundary instead
        // of spreading NaN/Inf through every downstream processor.
        if (!std::isfinite(gain) || gain == 1.0f) return;
        if (m_numChannels > 0) m_isDirty = true;
        hirari_audio_buffer_apply_gain(
            getArrayOfWritePointers(), m_numChannels, m_numSamples, gain);
    }

    uint32_t getNumChannels() const { return m_numChannels; }
    uint32_t getNumSamples() const { return m_numSamples; }
    const float* const* getArrayOfReadPointers() const {
        if (m_isExternal) return const_cast<const float* const*>(m_externalData);
        return const_cast<const float* const*>(
            hirari_audio_buffer_storage_channel_pointers(m_storage));
    }

    float** getArrayOfWritePointers() {
        if (m_isExternal) return m_externalData;
        return hirari_audio_buffer_storage_channel_pointers(m_storage);
    }

private:
    void updatePointers() {
        if (m_isExternal || !m_storage || m_data == nullptr) return;
        (void)hirari_audio_buffer_storage_update_channel_pointers(
            m_storage, m_data, m_numChannels, m_numSamples);
    }

    uint32_t m_numChannels;
    uint32_t m_numSamples;
    size_t m_capacity;
    void* m_storage = nullptr;
    bool m_isExternal = false;
    bool m_isDirty = true;
    float* m_data = nullptr;
    float** m_externalData = nullptr;
};

} // namespace Hirari::Core
