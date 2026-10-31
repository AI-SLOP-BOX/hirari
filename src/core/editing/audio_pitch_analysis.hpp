#pragma once

#include "../rust_ffi.hpp"

#include <algorithm>
#include <cmath>
#include <cstddef>
#include <memory>
#include <vector>

namespace hirari::editing {

// Bounded, offline monophonic F0 analysis. The FFT, autocorrelation, note
// boundary tracking, and pitch-anchor generation run in the Rust core.
class AudioPitchAnalyzer {
public:
    static constexpr std::size_t kMaxSegments = 4096;

    struct Config {
        double minHz = 65.0;
        double maxHz = 1200.0;
        std::size_t window = 2048;
        std::size_t hop = 512;
        double threshold = 0.82;
        double noteChangeThresholdCents = 90.0;
        std::size_t noteChangeConfirmationFrames = 2;
    };
};

} // namespace hirari::editing
