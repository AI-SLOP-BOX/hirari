#pragma once
#include <fstream>
#include <sstream>
#include <vector>
#include <string>
#include <iostream>
#include <algorithm>
#include <fcntl.h> 
#include <unistd.h>
#include <type_traits>
#include <cstring>
#include <cstdio>
#include <memory>
#include <cmath>
#include <limits>
#include <filesystem>
#include <atomic>
#include <unordered_set>
#include "engine/automation_curve.hpp"
#include "editing/audio_note_segment.hpp"
#include "editing/event_processing_history.hpp"

namespace Aura::Core {

/**
 * @class ProjectCipher
 * @brief Simple XOR-based obfuscation for project files.
 * HONEST FIX: Purged 'Fractal' branding. This is a basic cipher, not a fractal one.
 */
class ProjectCipher {
public:
    static void process(uint8_t* data, size_t size, uint64_t key) {
        for (size_t i = 0; i < size; ++i) {
            data[i] ^= static_cast<uint8_t>((key >> (i % 8)) & 0xFF);
        }
    }
};

/**
 * @class ProjectSerializer
 * @brief Manages binary project serialization and versioning.
 * HONEST FIX: Optimized CRC32 and purged 'Sovereign' marketing fluff.
 */
class ProjectSerializer {
#include "project_serializer_part_1.inc"
#include "project_serializer_part_2.inc"
#include "project_serializer_part_3.inc"
#include "project_serializer_part_4.inc"
};

} // namespace Aura::Core
