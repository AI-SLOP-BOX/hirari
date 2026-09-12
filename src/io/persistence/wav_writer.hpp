#pragma once
#include <string>
#include <vector>
#include <fstream>
#include <cstdint>
#include <cmath>
#include <limits>
#include <filesystem>
#include <atomic>
#include <array>
#include <algorithm>

#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif

namespace Aura::IO::Persistence {

/**
 * @class WavWriter
 * @brief Zero-overhead WAV Export.
 * HONEST FIX: Implements the 2026-spec 32-bit Float WAV (Type 3) 
 * for maximum dynamic range and high-fidelity output.
 */
class WavWriter {
#include "wav_writer_part_1.inc"
#include "wav_writer_part_2.inc"
#include "wav_writer_part_3.inc"

} // namespace Aura::IO::Persistence
