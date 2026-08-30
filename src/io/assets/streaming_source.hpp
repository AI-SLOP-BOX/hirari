#pragma once

#include <vector>
#include <atomic>
#include <fstream>
#include <mutex>
#include <algorithm>
#include <array>
#include <cmath>
#include <string>

#include "../../core/audio_region.hpp"

namespace Aura::Core::Assets {

class StreamingSource : public ::Aura::Core::IAudioSource {
public:
    float getSample(uint32_t channel, uint64_t sampleIdx) const override {
        if (channel >= m_numChannels || sampleIdx >= m_totalSamples) return 0.0f;

        // This accessor is deliberately cache-only: seeking a file from the
        // audio thread would violate the real-time contract. The streaming
        // worker publishes both buffer metadata and contents before the audio
        // side can observe the buffer.
        const size_t active = m_activeIdx.load(std::memory_order_acquire);
        m_readers[active].fetch_add(1, std::memory_order_acquire);
        if (active != m_activeIdx.load(std::memory_order_acquire)) {
            m_readers[active].fetch_sub(1, std::memory_order_release);
            const size_t retry = m_activeIdx.load(std::memory_order_acquire);
            m_readers[retry].fetch_add(1, std::memory_order_acquire);
            if (retry != m_activeIdx.load(std::memory_order_acquire)) {
                m_readers[retry].fetch_sub(1, std::memory_order_release);
                return 0.0f;
            }
            const uint64_t retryStart = m_bufferStart[retry].load(std::memory_order_acquire);
            const uint32_t retryFrames = m_bufferFrames[retry].load(std::memory_order_acquire);
            if (sampleIdx < retryStart || sampleIdx - retryStart >= retryFrames) {
                m_readers[retry].fetch_sub(1, std::memory_order_release);
                return 0.0f;
            }
            const size_t retryOffset = static_cast<size_t>(sampleIdx - retryStart) * m_numChannels + channel;
            const float retryValue = m_buffers[retry][retryOffset];
            m_readers[retry].fetch_sub(1, std::memory_order_release);
            return retryValue;
        }
        const uint64_t start = m_bufferStart[active].load(std::memory_order_acquire);
        const uint32_t frames = m_bufferFrames[active].load(std::memory_order_acquire);
        if (sampleIdx >= start && sampleIdx - start < frames) {
            const size_t offset = static_cast<size_t>(sampleIdx - start) * m_numChannels + channel;
            const float value = m_buffers[active][offset];
            m_readers[active].fetch_sub(1, std::memory_order_release);
            return std::isfinite(value) ? value : 0.0f;
        }
        m_readers[active].fetch_sub(1, std::memory_order_release);
        return 0.0f;
    }
    uint64_t getNumSamples() const override { return getTotalSamples(); }
    uint32_t getNumChannels() const override { return m_numChannels; }
public:
    static constexpr size_t kBufferSize = 65536; // 64k buffer

    StreamingSource(const std::string& filePath) : m_filePath(filePath) {
        m_file.open(filePath, std::ios::binary);
        if (m_file.is_open()) {
            m_file.seekg(0, std::ios::end);
            const std::streamoff bytes = m_file.tellg();
            const std::streamoff frameBytes = static_cast<std::streamoff>(sizeof(float) * m_numChannels);
            if (bytes > 0 && frameBytes > 0) {
                m_totalSamples = static_cast<uint64_t>(bytes / frameBytes);
                m_truncatedTail = (bytes % frameBytes) != 0;
            }
            m_file.seekg(0, std::ios::beg);
            for (auto& buffer : m_buffers) buffer.resize(kBufferSize * m_numChannels, 0.0f);

        // The first fill must target the active buffer. Writing the inactive
        // buffer here leaves the first audio block silent until a boundary.
            const uint32_t frames = readBuffer(0, 0);
            m_bufferStart[0].store(0, std::memory_order_release);
            m_bufferFrames[0].store(frames, std::memory_order_release);
            m_nextFileFrame = frames;
            m_needsRefill.store(m_nextFileFrame < m_totalSamples, std::memory_order_release);
        } else {
            m_sourceError.store(true, std::memory_order_release);
            m_sourceExhausted.store(true, std::memory_order_release);
        }
    }

    /**
     * @brief Checks if the background buffer needs refilling from disk.
     */
    bool needsRefill() const {
        return m_needsRefill.load(std::memory_order_acquire);
    }

    /**
     * @brief Performed by the IO Worker thread. Reads the next block into the inactive buffer.
     */
    void refill() {
        if (!m_file.is_open() || m_nextFileFrame >= m_totalSamples) {
            m_needsRefill.store(false, std::memory_order_release);
            if (m_nextFileFrame >= m_totalSamples) {
                m_sourceExhausted.store(true, std::memory_order_release);
            }
            return;
        }

        const size_t active = m_activeIdx.load(std::memory_order_acquire);
        const size_t inactiveIdx = 1 - active;
        if (m_readers[inactiveIdx].load(std::memory_order_acquire) != 0) return;
        const uint64_t start = m_nextFileFrame;
        const uint32_t frames = readBuffer(inactiveIdx, start);
        if (frames == 0) {
            m_needsRefill.store(false, std::memory_order_release);
            m_sourceExhausted.store(true, std::memory_order_release);
            return;
        }
        m_bufferStart[inactiveIdx].store(start, std::memory_order_release);
        m_bufferFrames[inactiveIdx].store(frames, std::memory_order_release);
        m_nextFileFrame += frames;
        m_needsRefill.store(false, std::memory_order_release);
    }

    /**
     * @brief Consumed by the Audio Thread.
     */
    float getNextSample() {
        // At most two attempts are needed for a double-buffer handoff. A
        // bounded retry is preferable to recursive re-entry on a rapidly
        // refilling source, which could otherwise exhaust the audio stack.
        for (int attempt = 0; attempt < 2; ++attempt) {
            size_t activeIdx = m_activeIdx.load(std::memory_order_acquire);
            uint32_t frames = m_bufferFrames[activeIdx].load(std::memory_order_acquire);
            if (m_readPos >= frames) {
                const size_t nextIdx = 1 - activeIdx;
                const uint32_t nextFrames = m_bufferFrames[nextIdx].load(std::memory_order_acquire);
                if (nextFrames == 0) {
                    m_needsRefill.store(true, std::memory_order_release);
                    return 0.0f;
                }
                m_activeIdx.store(nextIdx, std::memory_order_release);
                activeIdx = nextIdx;
                m_readPos = 0;
                frames = nextFrames;
            }

            m_readers[activeIdx].fetch_add(1, std::memory_order_acquire);
            if (activeIdx != m_activeIdx.load(std::memory_order_acquire)) {
                m_readers[activeIdx].fetch_sub(1, std::memory_order_release);
                continue;
            }
            const float sample = m_buffers[activeIdx][static_cast<size_t>(m_readPos) * m_numChannels];
            m_readers[activeIdx].fetch_sub(1, std::memory_order_release);
            ++m_readPos;
            if (m_readPos >= frames) m_needsRefill.store(true, std::memory_order_release);
            return std::isfinite(sample) ? sample : 0.0f;
        }
        m_needsRefill.store(true, std::memory_order_release);
        return 0.0f;
    }

    uint64_t getTotalSamples() const { return m_totalSamples; }
    bool isExhausted() const noexcept { return m_sourceExhausted.load(std::memory_order_acquire); }
    bool hasTruncatedTail() const noexcept { return m_truncatedTail; }
    bool isReady() const noexcept { return m_file.is_open() && m_totalSamples > 0; }
    bool hasSourceError() const noexcept { return m_sourceError.load(std::memory_order_acquire); }
    std::string getFilePath() const override { return m_filePath; }

private:
    uint32_t readBuffer(size_t bufferIndex, uint64_t startFrame) {
        if (!m_file.is_open() || bufferIndex >= m_buffers.size() || startFrame >= m_totalSamples) {
            if (bufferIndex < m_buffers.size()) {
                std::fill(m_buffers[bufferIndex].begin(), m_buffers[bufferIndex].end(), 0.0f);
            }
            return 0;
        }

        const uint64_t available = m_totalSamples - startFrame;
        const uint32_t frames = static_cast<uint32_t>(std::min<uint64_t>(kBufferSize, available));
        const size_t values = static_cast<size_t>(frames) * m_numChannels;
        m_file.clear();
        m_file.seekg(static_cast<std::streamoff>(startFrame * m_numChannels * sizeof(float)), std::ios::beg);
        m_file.read(reinterpret_cast<char*>(m_buffers[bufferIndex].data()),
                    static_cast<std::streamsize>(values * sizeof(float)));
        const std::streamsize readBytes = m_file.gcount();
        const uint32_t readFrames = static_cast<uint32_t>(std::max<std::streamsize>(
            0, std::min<std::streamsize>(readBytes / static_cast<std::streamsize>(m_numChannels * sizeof(float)), frames)));
        for (size_t i = 0; i < static_cast<size_t>(readFrames) * m_numChannels; ++i) {
            if (!std::isfinite(m_buffers[bufferIndex][i])) m_buffers[bufferIndex][i] = 0.0f;
        }
        std::fill(m_buffers[bufferIndex].begin() + static_cast<std::ptrdiff_t>(readFrames * m_numChannels),
                  m_buffers[bufferIndex].end(), 0.0f);
        return readFrames;
    }

    static constexpr uint32_t m_numChannels = 2;
    uint64_t m_totalSamples = 0;
    std::string m_filePath;
    std::ifstream m_file;
    
    std::array<std::vector<float>, 2> m_buffers;
    std::array<std::atomic<uint64_t>, 2> m_bufferStart{{0, 0}};
    std::array<std::atomic<uint32_t>, 2> m_bufferFrames{{0, 0}};
    mutable std::array<std::atomic<uint32_t>, 2> m_readers{{0, 0}};
    std::atomic<size_t> m_activeIdx{0};
    uint64_t m_readPos = 0;
    uint64_t m_nextFileFrame = 0;
    
    std::atomic<bool> m_needsRefill{false};
    std::atomic<bool> m_sourceExhausted{false};
    std::atomic<bool> m_sourceError{false};
    bool m_truncatedTail = false;
};

} // namespace Aura::Core::Assets
