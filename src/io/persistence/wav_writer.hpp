#pragma once
#include <string>
#include <vector>
#include <cstdint>
#include <filesystem>
#include <algorithm>
#include "../../core/rust_ffi.hpp"

namespace Hirari::IO::Persistence {

/**
 * @class WavWriter
 * @brief Compatibility API for Rust-owned WAV export and publication.
 * Encoding, streaming, and durable file publication are implemented in the
 * Rust core bridge; these classes preserve the native engine's existing API.
 */
class WavWriter {
#include "wav_writer_part_1.inc"
#include "wav_writer_part_2.inc"
#include "wav_writer_part_3.inc"

} // namespace Hirari::IO::Persistence
