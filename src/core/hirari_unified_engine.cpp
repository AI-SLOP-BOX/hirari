/* Hirari DAW Ultimate - (c) 2026 Hirari DAW Project */
#include "hirari_unified_engine.hpp"
#include "hirari-core-bridge/src/lib.rs.h"
#include "engine/video_engine.hpp"
#include "engine/video_system_bridge.hpp"
#include "engine/bus_track.hpp"
#include "engine/sidechain_manager.hpp"
#include "dsp/analysis/master_meter.hpp"
#include "dsp/effects/master_limiter.hpp"
#include "engine/track.hpp"
#include "concurrency/audio_task_manager.hpp"

#include "diagnostics/engine_diagnostics.hpp"
#include "diagnostics/forensic_journaler.hpp"
#include <thread>
#include "project_automation.hpp"
#include <cstring>
#include <cstdio>
#include <algorithm>
#include <array>
#include <iostream>
#include <chrono>
#include <filesystem>
#include <iterator>
#include <fcntl.h>
#include <unistd.h>
#include "concurrency/audio_task_manager.hpp"
#include "log_buffer.hpp"
#include "diagnostics_kernel.hpp"
#include "database/project_db.hpp"
#include "concurrency/forensic_scratchpad.hpp"
#include "rust_ffi.hpp"

namespace {

} // namespace

namespace Hirari::Core::Engine {

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
// Rust owns the tempo and time-signature snapshots; only the FFI shape lives
// here. The retry loop is the exact count/copy dance the deleted compatibility
// facade ran, kept in one place so a snapshot racing with a writer cannot
// truncate the vector the caller reads.
template <typename EventType, size_t (*Count)(const void*), size_t (*Copy)(const void*, void*, size_t)>
std::vector<EventType> readTempoSnapshot(const void* state) {
    std::vector<EventType> result(Count(state));
    while (true) {
        const size_t count = Copy(state, result.empty() ? nullptr : result.data(), result.size());
        if (count <= result.size()) {
            result.resize(count);
            return result;
        }
        result.resize(count);
    }
}

std::vector<HirariTempoEvent> readTempoEvents(const void* state) {
    return readTempoSnapshot<HirariTempoEvent, hirari_tempo_map_get_event_count,
                             hirari_tempo_map_copy_events>(state);
}

#include "hirari_unified_engine_part_1a.inc"
#include "hirari_unified_engine_part_1b.inc"
#include "hirari_unified_engine_part_2a.inc"
#include "hirari_unified_engine_part_2b.inc"
#include "hirari_unified_engine_part_3a.inc"
#include "hirari_unified_engine_part_3b.inc"
#include "hirari_unified_engine_part_4a.inc"
#include "hirari_unified_engine_part_4b.inc"
#include "hirari_unified_engine_part_4z.inc"
#include "hirari_unified_engine_part_5a.inc"
#include "hirari_unified_engine_part_5b.inc"
#include "hirari_unified_engine_part_6a.inc"
#include "hirari_unified_engine_part_6b.inc"
#include "hirari_unified_engine_part_7.inc"
} // namespace Hirari::Core::Engine
