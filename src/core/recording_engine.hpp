#pragma once

#include <vector>
#include <string>
#include <fstream>
#include <atomic>
#include <thread>
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <limits>
#include <filesystem>
#include <mutex>
#include <array>
#include <memory>
#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif
#include "audio_buffer.hpp"
#include "lock_free_ring_buffer.hpp"
#include "../io/persistence/wav_writer.hpp"

namespace Aura::Core {

/**
 * @class RecordingEngine
 * @brief Professional mastering-grade audio capture engine.
 * HONEST FIX: Implements 32-bit Float WAV recording with Lock-Free ring buffering 
 * and proper RIFF chunk management. Prevents recording glitches even under heavy CPU load.
 */
class RecordingEngine {
public:
    static constexpr size_t kBufferSize = 1048576; // float slots reserved for the SPSC queue
    static constexpr uint16_t kChannels = 2;
    static constexpr uint16_t kBitsPerSample = 32;
    static constexpr uint32_t kMaxDataSize = std::numeric_limits<uint32_t>::max();
    static constexpr size_t kFrameBufferSize = kBufferSize / kChannels;
    // The recorder reserves an RF64-capable header from the beginning.  Small
    // takes are still published as RIFF (with a JUNK reservation chunk), while
    // long takes can be finalized as RF64 without moving the audio payload.
    static constexpr uint64_t kReservedHeaderBytes = 80;
    static constexpr uint64_t kBytesPerFrame = sizeof(float) * kChannels;
    static constexpr uint64_t kMaxFrames =
        std::numeric_limits<uint64_t>::max() / kBytesPerFrame;

    struct StereoFrame {
        float left = 0.0f;
        float right = 0.0f;
    };

    RecordingEngine() : m_isRecording(false), m_stopThread(false), m_writeFailed(false) {}
    ~RecordingEngine() { stop(); }

    struct WaveHeader {
        char riff[4] = {'R', 'I', 'F', 'F'};
        uint32_t fileSize;
        char wave[4] = {'W', 'A', 'V', 'E'};
        char fmt[4] = {'f', 'm', 't', ' '};
        uint32_t fmtSize = 16;
        uint16_t format = 3; // 3 = IEEE Float
        uint16_t channels = 2;
        uint32_t sampleRate;
        uint32_t byteRate;
        uint16_t blockAlign;
        uint16_t bitsPerSample = 32;
        char data[4] = {'d', 'a', 't', 'a'};
        uint32_t dataSize;
    };

    bool start(const std::string& path, double sr) {
        std::lock_guard<std::mutex> lifecycleLock(m_lifecycleMutex);
        stopUnlocked();
        // stopUnlocked() has joined the sole consumer, so the SPSC queue is
        // quiescent here. Do not allow a failed/aborted take to bleed frames
        // into the next recording session.
        m_ringBuffer.reset();
        if (path.empty()) return false;
        if (!std::isfinite(sr) || sr <= 0.0 ||
            sr > static_cast<double>(std::numeric_limits<uint32_t>::max() /
                                     (kChannels * (kBitsPerSample / 8)))) {
            return false;
        }

        std::error_code ec;
        std::filesystem::path fsPath(path);
        if (fsPath.has_parent_path()) {
            std::filesystem::create_directories(fsPath.parent_path(), ec);
        }

        m_sampleRate = sr;
        m_streamWriter = std::make_unique<
            ::Aura::IO::Persistence::WavWriter::Float32StreamWriter>(
                path, static_cast<uint32_t>(sr), kChannels);
        if (!m_streamWriter->isOpen()) {
            m_streamWriter.reset();
            return false;
        }

        m_stopThread = false;
        m_writeFailed = false;
        m_bufferOverflowed = false;
        m_droppedFrames = 0;
        m_totalFramesWritten = 0;
        try {
            m_writerThread = std::thread(&RecordingEngine::writerWork, this);
        } catch (...) {
            m_stopThread.store(true, std::memory_order_release);
            m_streamWriter.reset();
            return false;
        }
        m_isRecording.store(true, std::memory_order_release);
        return true;
    }

    void stop() {
        std::lock_guard<std::mutex> lifecycleLock(m_lifecycleMutex);
        stopUnlocked();
    }

private:
    // Control-thread lifecycle only. The realtime write() path intentionally
    // never takes this mutex; start/stop are serialized before touching the
    // writer thread, stream, or temporary publication path.
    void stopUnlocked() {
        m_isRecording.store(false, std::memory_order_release);
        m_stopThread = true;
        m_wakeSequence.fetch_add(1, std::memory_order_release);
        m_wakeSequence.notify_one();
        if (m_writerThread.joinable()) m_writerThread.join();
        
        if (m_streamWriter) {
            if (!m_writeFailed.load(std::memory_order_acquire) &&
                !m_streamWriter->finish()) {
                m_writeFailed.store(true, std::memory_order_release);
            }
            m_streamWriter.reset();
        }
    }

public:

    /**
     * @brief Pushes incoming audio samples to the ring buffer.
     * RT-SAFE: No locks, no allocations.
     */
    bool write(const float* l, const float* r, uint32_t numSamples) {
        if (!m_isRecording.load(std::memory_order_acquire) || l == nullptr || r == nullptr) {
            return false;
        }
        if (numSamples == 0) return true;
        for (uint32_t s = 0; s < numSamples; ++s) {
            const StereoFrame frame{
                std::isfinite(l[s]) ? l[s] : 0.0f,
                std::isfinite(r[s]) ? r[s] : 0.0f
            };
            if (!m_ringBuffer.push(frame)) {
                m_bufferOverflowed.store(true, std::memory_order_release);
                m_droppedFrames.fetch_add(static_cast<uint64_t>(numSamples - s),
                                          std::memory_order_relaxed);
                return false;
            }
        }
        m_wakeSequence.fetch_add(1, std::memory_order_release);
        m_wakeSequence.notify_one();
        return true;
    }

    bool isRecording() const { return m_isRecording.load(); }
    bool hasWriteError() const { return m_writeFailed.load(std::memory_order_acquire); }
    bool hasBufferOverflowed() const { return m_bufferOverflowed.load(std::memory_order_acquire); }
    uint64_t droppedFrames() const noexcept { return m_droppedFrames.load(std::memory_order_acquire); }

private:
    void writerWork() {
        // High-priority disk writer thread
        while (m_isRecording.load(std::memory_order_acquire) || !m_stopThread.load(std::memory_order_relaxed)) {
            if (!drainBuffer()) {
                const uint64_t observed = m_wakeSequence.load(std::memory_order_acquire);
                if (m_isRecording.load(std::memory_order_acquire) ||
                    !m_stopThread.load(std::memory_order_relaxed)) {
                    m_wakeSequence.wait(observed, std::memory_order_acquire);
                }
            }
        }
        // Final drain after recording has stopped to prevent trailing cut-off.
        // A single drain only removes one batch (4096 frames); long or bursty
        // callbacks can leave several batches queued when stop() flips the
        // recording flag. Keep draining until the SPSC queue is empty.
        while (drainBuffer()) {
        }
        // The control thread patches the header and commits the temporary
        // file after the writer has drained it.
    }

    bool drainBuffer() {
        // Batch drain to minimize context switching and disk syscall overhead
        StereoFrame frameBatch[4096];
        uint32_t count = 0;
        
        const uint64_t remaining = kMaxFrames -
            std::min(m_totalFramesWritten.load(std::memory_order_relaxed), kMaxFrames);
        if (remaining == 0) {
            m_writeFailed.store(true, std::memory_order_release);
            m_isRecording.store(false, std::memory_order_release);
            m_stopThread.store(true, std::memory_order_release);
            return false;
        }
        const uint32_t limit = static_cast<uint32_t>(std::min<uint64_t>(4096, remaining));
        while (count < limit && m_ringBuffer.pop(frameBatch[count])) {
            count++;
        }

        if (count > 0) {
            float left[4096];
            float right[4096];
            for (uint32_t i = 0; i < count; ++i) {
                left[i] = frameBatch[i].left;
                right[i] = frameBatch[i].right;
            }
            const float* channels[kChannels] = {left, right};
            if (m_streamWriter && m_streamWriter->writeFrames(channels, count)) {
                m_totalFramesWritten += count;
            } else {
                m_writeFailed.store(true, std::memory_order_release);
                m_isRecording.store(false, std::memory_order_release);
                m_stopThread.store(true, std::memory_order_release);
            }
        }
        return count > 0;
    }

    std::atomic<bool> m_isRecording;
    std::atomic<bool> m_stopThread;
    std::atomic<bool> m_writeFailed;
    std::atomic<bool> m_bufferOverflowed{false};
    std::atomic<uint64_t> m_droppedFrames{0};
    std::atomic<uint64_t> m_totalFramesWritten{0};
    std::atomic<uint64_t> m_wakeSequence{0};
    std::mutex m_lifecycleMutex;
    double m_sampleRate = 44100.0;
    std::thread m_writerThread;
    std::unique_ptr<::Aura::IO::Persistence::WavWriter::Float32StreamWriter> m_streamWriter;
    LockFreeRingBuffer<StereoFrame, kFrameBufferSize> m_ringBuffer;
};

} // namespace Aura::Core
