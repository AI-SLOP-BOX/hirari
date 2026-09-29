#pragma once

#include "../../core/rust_ffi.hpp"

#include <cstddef>
#include <cstdint>

namespace Hirari::DSP::Analysis {

class SpectralAnalyzer {
public:
    static constexpr uint32_t kFFTSize = 4096;

    void analyze(const float* input, uint32_t size, float* magnitude_output) const {
        hirari_spectral_profile_analyze(input, size, magnitude_output);
    }
};

} // namespace Hirari::DSP::Analysis
