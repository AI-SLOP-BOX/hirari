/* Aura DAW Ultimate - (c) 2026 Aura DAW Project */
#include "aura_unified_engine.hpp"
#include "engine/video_engine.hpp"
#include "engine/video_system_bridge.hpp"
#include "engine/bus_track.hpp"
#include "engine/sidechain_manager.hpp"
#include "utils/wav_writer.hpp"
#include "dsp/AuraDSP.hpp"
#include "dsp/analysis/master_meter.hpp"
#include "dsp/effects/console_dsp.hpp"
#include "dsp/effects/master_limiter.hpp"
#include "engine/track.hpp"
#include "engine/process_graph.hpp"
#include "engine/pdc_manager.hpp"
#include "engine/tempo_map.hpp"
#include "concurrency/audio_task_manager.hpp"

#include "network/mobile_bridge_kernel.hpp"
#include "network/collaboration_hub_kernel.hpp"
#include "dsp/haptic_metadata.hpp"
#include "diagnostics/engine_diagnostics.hpp"
#include "diagnostics/forensic_journaler.hpp"
#include "composition/harmony_engine.hpp"
#include "engine/script_manager.hpp"
#include <thread>
#include "project_serializer.hpp"
#include <cstring>
#include <cstdio>
#include <algorithm>
#include <iostream>
#include <chrono>
#include <filesystem>
#include <iterator>
#include <cerrno>
#include <fcntl.h>
#include <sys/mman.h>
#include <unistd.h>
#include "concurrency/audio_task_manager.hpp"
#include "log_buffer.hpp"
#include "diagnostics_kernel.hpp"
#include "engine/tonal_sync.hpp"
#include "engine/script_manager.hpp"
#include "database/project_db.hpp"
#include "io/multi_device_sink.hpp"
#include "io/audio_decoder.hpp"
#include "dsp/spatial/ambisonic_7th_order_kernel.hpp"
#include <map>
#if defined(__x86_64__) || defined(_M_X64)
#include <xmmintrin.h>
#elif defined(__arm64__) || defined(__aarch64__)
#include <arm_neon.h>
#endif

#include "dsp/utils/forensic_timer.hpp"
#include "concurrency/forensic_scratchpad.hpp"

namespace {

struct ScopedFd {
    explicit ScopedFd(int value) : fd(value) {}
    ~ScopedFd() { if (fd >= 0) ::close(fd); }
    ScopedFd(const ScopedFd&) = delete;
    ScopedFd& operator=(const ScopedFd&) = delete;
    int release() noexcept { const int value = fd; fd = -1; return value; }
    int fd = -1;
};

struct ScopedMapping {
    ScopedMapping(void* value, size_t length) : data(value), size(length) {}
    ~ScopedMapping() {
        if (data != MAP_FAILED && data != nullptr && size > 0) ::munmap(data, size);
    }
    ScopedMapping(const ScopedMapping&) = delete;
    ScopedMapping& operator=(const ScopedMapping&) = delete;
    void* release() noexcept { const auto value = data; data = nullptr; size = 0; return value; }
    void* data = nullptr;
    size_t size = 0;
};

uint32_t read_le_u32(const std::vector<uint8_t>& bytes, size_t offset) {
    return static_cast<uint32_t>(bytes[offset]) |
           (static_cast<uint32_t>(bytes[offset + 1]) << 8u) |
           (static_cast<uint32_t>(bytes[offset + 2]) << 16u) |
           (static_cast<uint32_t>(bytes[offset + 3]) << 24u);
}

void append_le_u32(std::vector<uint8_t>& bytes, uint32_t value) {
    bytes.push_back(static_cast<uint8_t>(value & 0xffu));
    bytes.push_back(static_cast<uint8_t>((value >> 8u) & 0xffu));
    bytes.push_back(static_cast<uint8_t>((value >> 16u) & 0xffu));
    bytes.push_back(static_cast<uint8_t>((value >> 24u) & 0xffu));
}

} // namespace

namespace Aura::Core::Engine {

class EngineOrchestrator {
public:
    static EngineOrchestrator& getInstance() {
        static EngineOrchestrator instance;
        return instance;
    }
    void heartbeat() {
        // Telemetry monitoring heartbeat
    }
};

// Deterministic, lock-free entropy source for non-audio pattern generation.
// This is deliberately not used for security or cryptography; its purpose is
// to make generated fills vary without touching the audio callback or libc
// rand() state, while retaining reproducibility within a process.
#include "aura_unified_engine_part_1.inc"
#include "aura_unified_engine_part_2.inc"
#include "aura_unified_engine_part_3.inc"
#include "aura_unified_engine_part_4.inc"
#include "aura_unified_engine_part_5.inc"
#include "aura_unified_engine_part_6.inc"
#include "aura_unified_engine_part_7.inc"
} // namespace Aura::Core::Engine
