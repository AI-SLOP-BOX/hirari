#pragma once

#include <cstdint>
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Utils {

/** C++ compatibility handle for the Rust TPDF dither engine. */
class TPDFDither {
public:
    TPDFDither() : m_state(hirari_tpdf_dither_create()) {}
    ~TPDFDither() { hirari_tpdf_dither_destroy(m_state); }

    TPDFDither(const TPDFDither&) = delete;
    TPDFDither& operator=(const TPDFDither&) = delete;

    float process() { return hirari_tpdf_dither_next(m_state); }
    void processBlock(float* buffer, uint32_t numSamples) {
        if (buffer) hirari_tpdf_dither_process(m_state, buffer, numSamples);
    }

private:
    void* m_state = nullptr;
};

/** C++ compatibility handle for the Rust noise-shaped dither engine. */
class NoiseShapingDither {
public:
    NoiseShapingDither() : m_state(hirari_noise_shaping_dither_create()) {}
    ~NoiseShapingDither() { hirari_noise_shaping_dither_destroy(m_state); }

    NoiseShapingDither(const NoiseShapingDither&) = delete;
    NoiseShapingDither& operator=(const NoiseShapingDither&) = delete;

    float process(float sample, int bits = 16) {
        return hirari_noise_shaping_dither_process_sample(m_state, sample, bits);
    }
    void processBlock(float* buffer, uint32_t numSamples, int bits = 16) {
        if (buffer) hirari_noise_shaping_dither_process(m_state, buffer, numSamples, bits);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Utils
