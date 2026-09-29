#pragma once

#include <vector>
#include <algorithm>
#include <memory>
#include <atomic>
#include <new>
#include <limits>
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
    void lockMemory() {
#if !defined(_WIN32)
        if (m_data) (void)mlock(m_data, m_capacity * sizeof(float));
#else
        // Windows page locking requires process-specific privilege handling;
        // allocation remains valid even when physical pinning is unavailable.
        (void)m_data;
#endif
    }

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
            m_pointers = std::move(other.m_pointers);
            other.m_storage = nullptr;
            other.m_data = nullptr;
            other.m_externalData = nullptr;
            other.m_numChannels = 0;
            other.m_numSamples = 0;
            other.m_capacity = 0;
            other.m_isExternal = false;
            other.m_isDirty = true;
            other.m_pointers.clear();
            for (auto& pointer : other.m_staticPointers) pointer = nullptr;
            updatePointers();
        }
        return *this;
    }

    AudioBuffer(const AudioBuffer&) = delete;
    AudioBuffer& operator=(const AudioBuffer&) = delete;

    bool reserve(uint32_t channels, uint32_t samples) {
        if (samples > std::numeric_limits<uint32_t>::max() - 15u) {
            throw std::bad_array_new_length();
        }
        const uint32_t paddedSamples = (samples + 15u) & ~15u;
        if (channels != 0 && static_cast<size_t>(paddedSamples) >
                                 std::numeric_limits<size_t>::max() / channels) {
            throw std::bad_array_new_length();
        }
        size_t required = static_cast<size_t>(channels) * paddedSamples;
        const bool needsDataAllocation = required > m_capacity ||
                                         (m_isExternal && required != 0);
        const bool needsPointerAllocation =
            channels > kMaxFastPathChannels && m_pointers.size() != channels;

        if (needsDataAllocation || needsPointerAllocation) {
            // --- INDUSTRIAL SOVEREIGNTY: Zero-Allocation Guard ---
            if (m_rtGuard) {
                // Never perform stdio, formatting, or any other blocking I/O on
                // the audio callback.  Keep the detailed counters local and send
                // a fixed-size notification through the RT-safe status queue.
                m_rtResizeAttempts.fetch_add(1, std::memory_order_relaxed);
                m_lastRtRequestedCapacity.store(required, std::memory_order_relaxed);
                m_lastRtCapacity.store(m_capacity, std::memory_order_relaxed);
                StatusQueue::getInstance().pushFromAudio(
                    StatusQueue::Severity::Critical,
                    needsDataAllocation ? "AUDIO_BUFFER_RESIZE_ATTEMPTED"
                                        : "AUDIO_BUFFER_POINTER_RESIZE_ATTEMPTED");
                return false;
            }
            if (needsDataAllocation) {
                if (required > std::numeric_limits<size_t>::max() / sizeof(float)) {
                    throw std::bad_array_new_length();
                }
                if (!m_storage) m_storage = hirari_audio_buffer_storage_create();
                if (!m_storage || !hirari_audio_buffer_storage_reserve(m_storage, required)) {
                    throw std::bad_alloc();
                }
                m_data = hirari_audio_buffer_storage_data(m_storage);
                m_capacity = hirari_audio_buffer_storage_capacity(m_storage);
                m_isExternal = false;
            }
            if (needsPointerAllocation) {
                m_pointers.assign(channels, nullptr);
            }
        }
        updatePointers();
        return true;
    }

    static void setRTThread(bool isRT) noexcept { m_rtGuard = isRT; }

    static uint64_t realtimeResizeAttempts() noexcept {
        return m_rtResizeAttempts.load(std::memory_order_relaxed);
    }

    static size_t lastRealtimeRequestedCapacity() noexcept {
        return m_lastRtRequestedCapacity.load(std::memory_order_relaxed);
    }

    static size_t lastRealtimeCapacity() noexcept {
        return m_lastRtCapacity.load(std::memory_order_relaxed);
    }

    static void resetRealtimeResizeStats() noexcept {
        m_rtResizeAttempts.store(0, std::memory_order_relaxed);
        m_lastRtRequestedCapacity.store(0, std::memory_order_relaxed);
        m_lastRtCapacity.store(0, std::memory_order_relaxed);
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
        if (!channels || numChannels == 0 || numSamples == 0) {
            releaseOwnedData();
            m_numChannels = 0;
            m_numSamples = 0;
            m_isExternal = false;
            m_externalData = nullptr;
            return;
        }
        for (uint32_t channel = 0; channel < numChannels; ++channel) {
            if (channels[channel] == nullptr) {
                releaseOwnedData();
                m_numChannels = 0;
                m_numSamples = 0;
                m_isExternal = false;
                m_externalData = nullptr;
                return;
            }
        }
        releaseOwnedData();
        m_numChannels = numChannels;
        m_numSamples = numSamples;
        m_isExternal = true;
        m_externalData = channels;
        m_isDirty = true;
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
    bool isEmpty() const { return m_numSamples == 0 || m_numChannels == 0; }

    bool copyFrom(const float* l, const float* r, uint32_t numSamples) {
        if (l == nullptr || r == nullptr) return false;
        if (m_numChannels < 2 || m_numSamples < numSamples) {
            if (!resize(2, numSamples)) return false;
        }
        m_isDirty = true;
        return hirari_audio_buffer_copy(
            getWritePointer(0), getWritePointer(1), l, r, numSamples);
    }

    float getMagnitude(uint32_t channel) const {
        return getMagnitude(channel, 0, m_numSamples);
    }

    float getMagnitude(uint32_t channel, uint32_t start, uint32_t len) const {
        if (isEmpty() || channel >= m_numChannels || start >= m_numSamples) return 0.0f;
        uint32_t n = std::min(len, m_numSamples - start);
        return hirari_audio_buffer_magnitude(getReadPointer(channel, start), n);
    }

    const float* const* getArrayOfReadPointers() const {
        if (m_isExternal) return const_cast<const float* const*>(m_externalData);
        if (m_numChannels <= 2) return const_cast<const float* const*>(m_staticPointers);
        return const_cast<const float* const*>(m_pointers.data());
    }

    float** getArrayOfWritePointers() {
        if (m_isExternal) return m_externalData;
        if (m_numChannels <= 2) return m_staticPointers;
        return m_pointers.data();
    }

private:
    void releaseOwnedData() noexcept {
        hirari_audio_buffer_storage_release(m_storage);
        m_data = nullptr;
        m_capacity = 0;
    }

    void updatePointers() {
        if (m_data == nullptr && !m_isExternal) {
            for (auto& pointer : m_staticPointers) pointer = nullptr;
            std::fill(m_pointers.begin(), m_pointers.end(), nullptr);
            return;
        }
        for (auto& pointer : m_staticPointers) pointer = nullptr;
        for (uint32_t c = 0; c < std::min(m_numChannels, kMaxFastPathChannels); ++c) {
            m_staticPointers[c] = m_isExternal
                                      ? m_externalData[c]
                                      : m_data + (c * m_numSamples);
        }
        if (m_numChannels > kMaxFastPathChannels) {
            for (uint32_t c = 0; c < m_numChannels; ++c) {
                m_pointers[c] = m_isExternal
                                    ? m_externalData[c]
                                    : m_data + (c * m_numSamples);
            }
        }
    }

    uint32_t m_numChannels;
    uint32_t m_numSamples;
    size_t m_capacity;
    void* m_storage = nullptr;
    bool m_isExternal = false;
    bool m_isDirty = true;
    float* m_data = nullptr;
    float** m_externalData = nullptr;
    std::vector<float*> m_pointers;
    float* m_staticPointers[kMaxFastPathChannels] = {nullptr};
    // RT status is a property of the executing thread, not global process
    // state. UI and disk workers must remain free to allocate independently.
    static inline thread_local bool m_rtGuard = false;
    static inline std::atomic<uint64_t> m_rtResizeAttempts{0};
    static inline std::atomic<size_t> m_lastRtRequestedCapacity{0};
    static inline std::atomic<size_t> m_lastRtCapacity{0};
};

} // namespace Hirari::Core
