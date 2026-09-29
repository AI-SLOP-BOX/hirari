#pragma once

#include "../effects/fet_compressor.hpp"

namespace Hirari::DSP::Mixing {

// Keep the legacy Mixing namespace source-compatible while sharing the
// Rust-backed processor implementation used by the effects registry.
using FETCompressor = ::Hirari::DSP::Effects::FETCompressor;

} // namespace Hirari::DSP::Mixing
