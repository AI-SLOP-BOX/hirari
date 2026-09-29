#pragma once

#include <cstdint>
#include <string>
#include "rust_ffi.hpp"

namespace Hirari::Core {

// Legacy engine facade. The SPSC capture queue, disk worker, and WAV/RF64
// writer are owned by the Rust recording runtime.
class RecordingEngine {
public:
    static constexpr uint16_t kChannels = 2;

    RecordingEngine() : m_state(hirari_recording_capture_create()) {}
    ~RecordingEngine() { hirari_recording_capture_destroy(m_state); }

    RecordingEngine(const RecordingEngine&) = delete;
    RecordingEngine& operator=(const RecordingEngine&) = delete;

    bool start(const std::string& path, double sample_rate) {
        return hirari_recording_capture_start(m_state, path.c_str(), sample_rate);
    }

    void stop() { hirari_recording_capture_stop(m_state); }

    // Audio-thread entry point: Rust validates, sanitizes, and enqueues the
    // stereo frames without taking a lock or allocating.
    bool write(const float* left, const float* right, uint32_t frames) {
        return hirari_recording_capture_write(m_state, left, right, frames);
    }

    bool isRecording() const {
        return hirari_recording_capture_is_recording(m_state);
    }

    bool hasWriteError() const {
        return hirari_recording_capture_has_write_error(m_state);
    }

    bool hasBufferOverflowed() const {
        return hirari_recording_capture_has_overflowed(m_state);
    }

    uint64_t droppedFrames() const noexcept {
        return hirari_recording_capture_dropped_frames(m_state);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core
