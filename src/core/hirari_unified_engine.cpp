/* Hirari DAW Ultimate - (c) 2026 Hirari DAW Project */
#include "hirari_unified_engine.hpp"
#include "hirari-core-bridge/src/lib.rs.h"
#include "concurrency/audio_task_manager.hpp"

#include <thread>
#include <cstring>
#include <cstdio>
#include <algorithm>
#include <array>
#include <iostream>
#include <chrono>
#include <iterator>
#include <fcntl.h>
#include <unistd.h>
#include "rust_ffi.hpp"

namespace Hirari::Core::Engine {

#include "hirari_unified_engine_part_1a.inc"
#include "hirari_unified_engine_part_1b.inc"
#include "hirari_unified_engine_part_2a.inc"
#include "hirari_unified_engine_part_2b.inc"
#include "hirari_unified_engine_part_3a.inc"
#include "hirari_unified_engine_part_3b.inc"
#include "hirari_unified_engine_part_4a.inc"
#include "hirari_unified_engine_part_4z.inc"
#include "hirari_unified_engine_part_5a.inc"
#include "hirari_unified_engine_part_5b.inc"
#include "hirari_unified_engine_part_6a.inc"
#include "hirari_unified_engine_part_6b.inc"
} // namespace Hirari::Core::Engine
