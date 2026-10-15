#pragma once

#include <cstdint>
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Utils {

/**
 * Fixed seeds for the two dither channels of an offline render.
 *
 * They are constants rather than clock-derived values because a bounce has to
 * be reproducible: rendering the same project twice must yield the same file,
 * otherwise a user cannot compare two exports and no checksum of a render can
 * be verified. They are two distinct constants so the left and right noise
 * sequences are not correlated.
 */
inline constexpr uint32_t kTpdfDitherLeftSeed = 0x1234'5678;
inline constexpr uint32_t kTpdfDitherRightSeed = 0x9E37'79B9;

/** C++ compatibility handle for the Rust TPDF dither engine. */
class TPDFDither {
public:
    TPDFDither() : m_state(hirari_tpdf_dither_create()) {}
    explicit TPDFDither(uint32_t seed) : m_state(hirari_tpdf_dither_create_with_seed(seed)) {}
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
