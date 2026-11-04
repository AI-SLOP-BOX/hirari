#pragma once

#include <vector>
#include <memory>
#include <atomic>
#include <chrono>
#include <string_view>
#include <array>
#include "bridge_types.hpp"
#include "concurrency/audio_task_manager.hpp"
#include <optional>
#include "engine/routing_engine.hpp"
#include "engine/bus_track.hpp"
#include <unordered_map>
#include <unordered_set>
#include <mutex>
#include <map>
#include <algorithm>
#include <iterator>
#include <cmath>
#include <cctype>
#include <fstream>
#include <filesystem>
#include <ostream>
#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif

#include "rust_ffi.hpp"
#include <thread>
#include <future>

namespace Hirari::Core::Engine {

class Track;

struct EngineConfig {
    float tempo;
    uint32_t timeSigNum;
    uint32_t timeSigDen;
    uint32_t sampleRate;
    uint32_t blockSize;
};

class HirariUnifiedEngine {
#include "hirari_unified_engine_decl_part_1.inc"
#include "hirari_unified_engine_decl_part_2.inc"
#include "hirari_unified_engine_decl_part_3.inc"
#include "hirari_unified_engine_decl_part_4.inc"
#include "hirari_unified_engine_decl_part_5.inc"
#include "hirari_unified_engine_decl_part_6.inc"
#include "hirari_unified_engine_decl_part_7.inc"
#include "hirari_unified_engine_decl_part_8.inc"

} // namespace Hirari::Core::Engine
