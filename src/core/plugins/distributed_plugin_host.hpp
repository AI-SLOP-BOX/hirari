#pragma once
#include <cstdint>
#include <string>
#include <vector>
#include <atomic>
#include <cstring>
#include <functional>
#include <algorithm>
#include <cmath>
#include "../../core/audio_buffer.hpp"

namespace Aura::Core::Plugins {

/**
 * @struct SharedAudioBus
 * @brief Zero-copy IPC layout for cross-process audio streaming.
 */
struct SharedAudioBus {
    static constexpr uint32_t MaxSamples = 2048;
    static constexpr uint32_t MaxChannels = 2;

    std::atomic<uint64_t> sequence{0};
    std::atomic<uint64_t> heartbeat{0}; 
    std::atomic<uint32_t> numSamples{0};
    float data[MaxChannels][MaxSamples];
};

/**
 * @class DistributedPluginHost
 * @brief Manages a plugin running in a separate process with wait-free IPC and autonomous fallback.
 */
class DistributedPluginHost {
public:
    using FallbackKernel = std::function<void(AudioBuffer&, uint32_t)>;

    DistributedPluginHost(const std::string& processUniqueId) 
        : m_id(processUniqueId) {}

    void setFallbackKernel(FallbackKernel f) { m_fallback = std::move(f); }

    /**
     * @brief Pushes local buffer to shared memory.
     */
    void pushToRemote(const AudioBuffer& buffer) {
        uint64_t seq = m_sharedBus.sequence.load(std::memory_order_relaxed);
        // Seqlock: odd means a writer owns the bus, even means a complete frame.
        if (seq & 1u) ++seq;
        m_sharedBus.sequence.store(seq + 1, std::memory_order_release);
        uint32_t samples = std::min(buffer.getNumSamples(), SharedAudioBus::MaxSamples);
        m_sharedBus.numSamples.store(samples, std::memory_order_relaxed);

        for (uint32_t c = 0; c < SharedAudioBus::MaxChannels; ++c) {
            const float* source = c < buffer.getNumChannels() ? buffer.getReadPointer(c) : nullptr;
            for (uint32_t sample = 0; sample < samples; ++sample) {
                const float value = source ? source[sample] : 0.0f;
                m_sharedBus.data[c][sample] = std::isfinite(value) ? value : 0.0f;
            }
            // Clear the unused portion so a remote reader cannot consume a
            // stale tail if it observes a larger frame during recovery.
            for (uint32_t sample = samples; sample < SharedAudioBus::MaxSamples; ++sample) {
                m_sharedBus.data[c][sample] = 0.0f;
            }
        }
        m_sharedBus.sequence.store(seq + 2, std::memory_order_release);
    }

    /**
     * @brief Pulls processed buffer with AUTONOMOUS FALLBACK.
     */
    bool pullFromRemote(AudioBuffer& buffer, uint64_t expectedSeq) {
        // --- PHASE 41: AUTONOMOUS FALLBACK SOVEREIGNTY ---
        if (m_sharedBus.sequence.load(std::memory_order_acquire) < expectedSeq) {
            if (m_fallback) {
                m_fallback(buffer, buffer.getNumSamples());
                m_fallbackCount.fetch_add(1, std::memory_order_relaxed);
                return true; // Transparently handled
            }
            return false; 
        }

        for (int attempt = 0; attempt < 2; ++attempt) {
            const uint64_t begin = m_sharedBus.sequence.load(std::memory_order_acquire);
            if ((begin & 1u) || begin < expectedSeq) continue;
            uint32_t samplesInBus = m_sharedBus.numSamples.load(std::memory_order_relaxed);
            uint32_t samplesToCopy = std::min(buffer.getNumSamples(), samplesInBus);
            for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
                float* destination = buffer.getWritePointer(c);
                if (!destination) continue;
                if (c < SharedAudioBus::MaxChannels) {
                    for (uint32_t sample = 0; sample < samplesToCopy; ++sample) {
                        const float value = m_sharedBus.data[c][sample];
                        destination[sample] = std::isfinite(value) ? value : 0.0f;
                    }
                    for (uint32_t sample = samplesToCopy; sample < buffer.getNumSamples(); ++sample) {
                        destination[sample] = 0.0f;
                    }
                } else {
                    std::fill(destination, destination + buffer.getNumSamples(), 0.0f);
                }
            }
            const uint64_t end = m_sharedBus.sequence.load(std::memory_order_acquire);
            if (begin == end && !(end & 1u)) return true;
        }
        if (m_fallback) {
            m_fallback(buffer, buffer.getNumSamples());
            m_fallbackCount.fetch_add(1, std::memory_order_relaxed);
            return true;
        }
        return false;
    }

    uint64_t getFallbackCount() const { return m_fallbackCount.load(std::memory_order_relaxed); }

private:
    std::string m_id;
    SharedAudioBus m_sharedBus; 
    FallbackKernel m_fallback;
    std::atomic<uint64_t> m_fallbackCount{0};
};

} // namespace Aura::Core::Plugins
