#pragma once
#include <vector>
#include <memory>
#include <atomic>
#include <mutex>
#include "../audio_buffer.hpp"
#include "lock_free.hpp"

namespace Hirari::Core::Concurrency {

/**
 * @class AudioBufferPool
 * @brief High-Performance Lock-Free Buffer Pool.
 * HONEST FIX: Replaced O(N) linear scans with O(1) Lock-Free Stack.
 * Eliminates cache contention during parallel track processing.
 */
class AudioBufferPool {
public:
    static AudioBufferPool& getInstance() {
        static AudioBufferPool instance;
        return instance;
    }

    void init(uint32_t numBuffers, uint32_t numSamples) {
        std::lock_guard<std::mutex> lock(m_initMutex);
        m_allBuffers.clear();
        
        for (uint32_t i = 0; i < numBuffers; ++i) {
            auto buf = std::make_unique<AudioBuffer>(2, numSamples);
            m_allBuffers.push_back(std::move(buf));
            m_availableQueue.push(m_allBuffers.back().get());
        }
    }

    AudioBuffer* acquire() {
        auto buf = m_availableQueue.pop();
        return buf ? *buf : nullptr;
    }

    void release(AudioBuffer* buffer) {
        if (buffer) m_availableQueue.push(buffer);
    }

private:
    AudioBufferPool() = default;
    std::mutex m_initMutex;
    std::vector<std::unique_ptr<AudioBuffer>> m_allBuffers;
    MPMCQueue<AudioBuffer*> m_availableQueue; 
};

} // namespace Hirari::Core::Concurrency
