#pragma once
#include <sys/mman.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/stat.h>
#include <string>
#include <cstring>
#include <atomic>
#include <cstdint>
#include <new>
#include "../core/plugins/midi_fragment_transport.hpp"

namespace Aura::Network {

/**
 * @class SharedMemoryBridge
 * @brief Zero-latency IPC bridge for Aura DAW side-car applications.
 * INDUSTRIAL: Mirrors engine telemetry and state into a POSIX shared memory segment.
 * This allows external UI processes (mirrors/tablets) to read levels without network overhead.
 */
class SharedMemoryBridge {
public:
    static constexpr uint32_t kSharedStateProtocolVersion = 1u;
    /**
     * @struct SharedState
     * @brief The data structure mirrored to shared memory.
     */
    struct SharedState {
        std::atomic<uint32_t> protocolVersion{0};
        std::atomic<float> peaksL[256];
        std::atomic<float> peaksR[256];
        std::atomic<float> cpuLoad;
        std::atomic<uint32_t> playhead;
        std::atomic<bool> isPlaying;
        // Extended MIDI lives beside telemetry so both processes map one
        // fixed-size region; the ring contains no process-local pointers.
        Aura::Core::Plugins::MidiExtendedMessageRing extendedMidi;
    };

    static SharedMemoryBridge& getInstance() {
        static SharedMemoryBridge instance;
        return instance;
    }

    /**
     * @brief Initializes the shared memory segment.
     */
    bool start(const std::string& segmentName = {}) {
        if (m_state || m_fd != -1) stop();
        static std::atomic<uint64_t> sequence{0};
        m_segmentName = segmentName.empty()
            ? "/aura_sovereign_bridge_" + std::to_string(
                  static_cast<unsigned long long>(::getpid())) + "_" +
                  std::to_string(sequence.fetch_add(1, std::memory_order_relaxed) + 1)
            : segmentName;
        m_fd = shm_open(m_segmentName.c_str(), O_CREAT | O_EXCL | O_RDWR, 0600);
        if (m_fd == -1) return false;

        if (ftruncate(m_fd, sizeof(SharedState)) == -1) {
            close(m_fd);
            m_fd = -1;
            shm_unlink(m_segmentName.c_str());
            m_segmentName.clear();
            return false;
        }

        void* ptr = mmap(0, sizeof(SharedState), PROT_READ | PROT_WRITE, MAP_SHARED, m_fd, 0);
        if (ptr == MAP_FAILED) {
            close(m_fd);
            m_fd = -1;
            shm_unlink(m_segmentName.c_str());
            m_segmentName.clear();
            return false;
        }

        // SharedState now contains atomics and a bounded ring. Construct it
        // in-place instead of byte-clearing non-trivial synchronization
        // objects, which would violate their initialization requirements.
        m_state = ::new (ptr) SharedState{};
        m_state->protocolVersion.store(kSharedStateProtocolVersion, std::memory_order_release);
        m_ownsState = true;
        return true;
    }

    // Maps a segment created by another process. The consumer must never
    // reconstruct or destroy SharedState; the creator owns its lifetime.
    bool openExisting(const std::string& segmentName) {
        if (segmentName.empty()) return false;
        if (m_state || m_fd != -1) stop();

        const int fd = shm_open(segmentName.c_str(), O_RDWR, 0600);
        if (fd == -1) return false;

        struct stat info{};
        if (fstat(fd, &info) == -1 || static_cast<std::size_t>(info.st_size) < sizeof(SharedState)) {
            close(fd);
            return false;
        }

        void* ptr = mmap(nullptr, sizeof(SharedState), PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        if (ptr == MAP_FAILED) {
            close(fd);
            return false;
        }

        m_fd = fd;
        m_state = static_cast<SharedState*>(ptr);
        if (m_state->protocolVersion.load(std::memory_order_acquire) !=
            kSharedStateProtocolVersion) {
            munmap(m_state, sizeof(SharedState));
            m_state = nullptr;
            close(fd);
            return false;
        }
        m_segmentName = segmentName;
        m_ownsState = false;
        return true;
    }

    const std::string& segmentName() const noexcept { return m_segmentName; }

    /**
     * @brief Updates the shared state from the engine thread.
     */
    void updateTelemetry(const float* l, const float* r, uint32_t count, float cpu) {
        if (!m_state) return;

        for (uint32_t i = 0; i < count && i < 256; ++i) {
            m_state->peaksL[i].store(l[i], std::memory_order_relaxed);
            m_state->peaksR[i].store(r[i], std::memory_order_relaxed);
        }
        m_state->cpuLoad.store(cpu, std::memory_order_relaxed);
    }

    /**
     * @brief Synchronizes transport state.
     */
    void updateTransport(uint32_t playhead, bool playing) {
        if (!m_state) return;
        m_state->playhead.store(playhead, std::memory_order_relaxed);
        m_state->isPlaying.store(playing, std::memory_order_relaxed);
    }

    bool pushExtendedMidi(uint64_t sampleOffset, uint8_t articulationId,
                          const uint8_t* bytes, std::size_t size) noexcept {
        return m_state && m_state->extendedMidi.push(
            sampleOffset, articulationId, bytes, size);
    }

    bool popExtendedMidi(Aura::Core::Plugins::MidiExtendedMessageRing::Message& message) noexcept {
        return m_state && m_state->extendedMidi.pop(message);
    }

    std::size_t pendingExtendedMidi() const noexcept {
        return m_state ? m_state->extendedMidi.size() : 0;
    }

    /**
     * @brief Closes and unlinks the shared memory segment.
     */
    void stop() {
        if (m_state) {
            if (m_ownsState) m_state->~SharedState();
            munmap(m_state, sizeof(SharedState));
            m_state = nullptr;
        }
        if (m_fd != -1) {
            close(m_fd);
            m_fd = -1;
        }
        if (m_ownsState && !m_segmentName.empty()) {
            shm_unlink(m_segmentName.c_str());
        }
        m_segmentName.clear();
        m_ownsState = false;
    }

private:
    SharedMemoryBridge() : m_fd(-1), m_state(nullptr), m_ownsState(false) {}
    ~SharedMemoryBridge() { stop(); }

    int m_fd;
    SharedState* m_state;
    bool m_ownsState;
    std::string m_segmentName;
};

} // namespace Aura::Network
