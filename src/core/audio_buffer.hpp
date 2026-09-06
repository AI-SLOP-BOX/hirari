#pragma once

#include <vector>
#include <algorithm>
#include <memory>
#include <cstring>
#include <atomic>
#include <new>
#include <cstdlib>
#include <limits>
#include <cmath>
#include "status_queue.hpp"
#if defined(_WIN32)
#include <malloc.h>
#else
#include <sys/mman.h>
#endif
#if defined(__arm64__) || defined(__aarch64__)
#include <arm_neon.h>
#elif defined(__x86_64__) || defined(_M_X64)
#include <immintrin.h>
#endif

namespace Aura::Core {

/**
 * @brief AudioBuffer: The 'Blood' of the DAW.
 * HONEST FIX: Unified the memory allocation into a single contiguous block.
 */
class AudioBuffer {
public:
    static constexpr uint32_t kMaxFastPathChannels = 16;

    AudioBuffer() : m_numChannels(0), m_numSamples(0), m_capacity(0), m_isExternal(false), m_data(nullptr) {}
    
    AudioBuffer(uint32_t channels, uint32_t samples) : AudioBuffer() {
        resize(channels, samples);
    }

    ~AudioBuffer() { releaseOwnedData(); }

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
            releaseOwnedData();
            m_numChannels = other.m_numChannels;
            m_numSamples = other.m_numSamples;
            m_capacity = other.m_capacity;
            m_data = other.m_data;
            m_isExternal = other.m_isExternal;
            m_externalData = other.m_externalData;
            m_isDirty = other.m_isDirty;
            m_pointers = std::move(other.m_pointers);
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
                void* raw = allocateAligned(required * sizeof(float));
                if (raw == nullptr) {
                    throw std::bad_alloc();
                }
                releaseOwnedData();
                m_data = static_cast<float*>(raw);
                m_capacity = required;
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
        if (m_isExternal) {
            for (uint32_t c = 0; c < m_numChannels; ++c) {
                float* p = m_externalData[c] + offset;
                uint32_t i = 0;
#if defined(__arm64__) || defined(__aarch64__)
                float32x4_t zero = vdupq_n_f32(0.0f);
                for (; i + 15 < n; i += 16) {
                    vst1q_f32(p + i, zero); vst1q_f32(p + i + 4, zero);
                    vst1q_f32(p + i + 8, zero); vst1q_f32(p + i + 12, zero);
                }
#elif defined(__x86_64__) || defined(_M_X64)
#if defined(__AVX512F__)
                __m512 zero = _mm512_setzero_ps();
                for (; i + 15 < n; i += 16) _mm512_storeu_ps(p + i, zero);
#else
                __m256 zero = _mm256_setzero_ps();
                for (; i + 7 < n; i += 8) _mm256_storeu_ps(p + i, zero);
#endif
#endif
                for (; i < n; ++i) p[i] = 0.0f;
            }
        } else if (m_data) {
            for (uint32_t c = 0; c < m_numChannels; ++c) {
                std::memset(m_data + (static_cast<size_t>(c) * m_numSamples) + offset, 0, static_cast<size_t>(n) * sizeof(float));
            }
        }
        m_isDirty = false;
    }

    // Boundary sanitizer for third-party DSP.  A plugin is not allowed to
    // poison the rest of the graph with NaN/Inf; keep the operation allocation
    // free so it is safe on the audio thread.
    uint32_t sanitizeNonFinite() noexcept {
        uint32_t replaced = 0;
        for (uint32_t channel = 0; channel < m_numChannels; ++channel) {
            float* samples = getWritePointer(channel);
            if (!samples) continue;
            for (uint32_t sample = 0; sample < m_numSamples; ++sample) {
                if (!std::isfinite(samples[sample])) {
                    samples[sample] = 0.0f;
                    ++replaced;
                }
            }
        }
        return replaced;
    }

    void addFrom(const AudioBuffer& other, uint32_t numSamples) {
        uint32_t channels = std::min(m_numChannels, other.getNumChannels());
        uint32_t n = std::min(numSamples, std::min(m_numSamples, other.getNumSamples()));

        for (uint32_t c = 0; c < channels; ++c) {
            float* dst = getWritePointer(c);
            const float* src = other.getReadPointer(c);
            uint32_t i = 0;

#if defined(__x86_64__) || defined(_M_X64)
#if defined(__AVX2__)
            for (; i + 7 < n; i += 8) {
                _mm256_storeu_ps(dst + i, _mm256_add_ps(_mm256_loadu_ps(dst + i), _mm256_loadu_ps(src + i)));
            }
#endif
#elif defined(__arm64__) || defined(__aarch64__)
            for (; i + 3 < n; i += 4) {
                vst1q_f32(dst + i, vaddq_f32(vld1q_f32(dst + i), vld1q_f32(src + i)));
            }
#endif
            for (; i < n; ++i) dst[i] += src[i];
        }
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

        uint32_t i = 0;
#if defined(__x86_64__) || defined(_M_X64)
    #if defined(__AVX2__)
        if (dR) {
            for (; i + 7 < n; i += 8) {
                _mm256_storeu_ps(dL + i, _mm256_add_ps(_mm256_loadu_ps(dL + i), _mm256_loadu_ps(srcL + i)));
                _mm256_storeu_ps(dR + i, _mm256_add_ps(_mm256_loadu_ps(dR + i), _mm256_loadu_ps(srcR + i)));
            }
        } else {
            for (; i + 7 < n; i += 8) {
                _mm256_storeu_ps(dL + i, _mm256_add_ps(_mm256_loadu_ps(dL + i), _mm256_loadu_ps(srcL + i)));
            }
        }
    #endif
#elif defined(__arm64__) || defined(__aarch64__)
        if (dR) {
            for (; i + 3 < n; i += 4) {
                vst1q_f32(dL + i, vaddq_f32(vld1q_f32(dL + i), vld1q_f32(srcL + i)));
                vst1q_f32(dR + i, vaddq_f32(vld1q_f32(dR + i), vld1q_f32(srcR + i)));
            }
        } else {
            for (; i + 3 < n; i += 4) {
                vst1q_f32(dL + i, vaddq_f32(vld1q_f32(dL + i), vld1q_f32(srcL + i)));
            }
        }
#endif
        for (; i < n; ++i) {
            dL[i] += srcL[i];
            if (dR) dR[i] += srcR[i];
        }
    }

    void applyGain(float gain) {
        // Gain values can arrive from automation or an external control
        // surface. Reject non-finite values at the buffer boundary instead
        // of spreading NaN/Inf through every downstream processor.
        if (!std::isfinite(gain) || gain == 1.0f) return;
        for (uint32_t c = 0; c < m_numChannels; ++c) {
            float* p = getWritePointer(c);
            for (uint32_t i = 0; i < m_numSamples; ++i) p[i] *= gain;
        }
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
        std::memcpy(getWritePointer(0), l, numSamples * sizeof(float));
        std::memcpy(getWritePointer(1), r, numSamples * sizeof(float));
        return true;
    }

    float getMagnitude(uint32_t channel) const {
        return getMagnitude(channel, 0, m_numSamples);
    }

    float getMagnitude(uint32_t channel, uint32_t start, uint32_t len) const {
        if (isEmpty() || channel >= m_numChannels || start >= m_numSamples) return 0.0f;
        uint32_t n = std::min(len, m_numSamples - start);
        const float* p = getReadPointer(channel) + start;
        float maxVal = 0.0f;
        uint32_t i = 0;

#if defined(__arm64__) || defined(__aarch64__)
        float32x4_t vMax = vdupq_n_f32(0.0f);
        for (; i + 15 < n; i += 16) {
            float32x4_t v0 = vabsq_f32(vld1q_f32(p + i));
            float32x4_t v1 = vabsq_f32(vld1q_f32(p + i + 4));
            float32x4_t v2 = vabsq_f32(vld1q_f32(p + i + 8));
            float32x4_t v3 = vabsq_f32(vld1q_f32(p + i + 12));
            vMax = vmaxq_f32(vMax, vmaxq_f32(vmaxq_f32(v0, v1), vmaxq_f32(v2, v3)));
        }
        maxVal = vmaxvq_f32(vMax);
#elif defined(__x86_64__) || defined(_M_X64)
    #if defined(__AVX__)
        __m256 vMax = _mm256_setzero_ps();
        __m256 absMask = _mm256_set1_ps(-0.0f);
        for (; i + 15 < n; i += 16) {
            __m256 v0 = _mm256_andnot_ps(absMask, _mm256_loadu_ps(p + i));
            __m256 v1 = _mm256_andnot_ps(absMask, _mm256_loadu_ps(p + i + 8));
            vMax = _mm256_max_ps(vMax, _mm256_max_ps(v0, v1));
        }
        alignas(32) float tmp[8]; _mm256_storeu_ps(tmp, vMax);
        for(int k=0; k<8; ++k) maxVal = std::max(maxVal, tmp[k]);
    #endif
#endif
        for (; i < n; ++i) maxVal = std::max(maxVal, std::abs(p[i]));
        return maxVal;
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
    static void* allocateAligned(size_t bytes) noexcept {
        if (bytes == 0) return nullptr;
#if defined(_WIN32)
        return _aligned_malloc(bytes, 4096);
#else
        void* raw = nullptr;
        return posix_memalign(&raw, 4096, bytes) == 0 ? raw : nullptr;
#endif
    }

    static void freeAligned(void* data) noexcept {
#if defined(_WIN32)
        _aligned_free(data);
#else
        std::free(data);
#endif
    }

    void releaseOwnedData() noexcept {
        if (!m_isExternal && m_data) freeAligned(m_data);
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

} // namespace Aura::Core
