#pragma once

#include <complex>
#include <cstddef>
#include <vector>
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Utils {

/** C++ API for the Rust radix-2 FFT with cached twiddle factors. */
class FFTUtils {
public:
    static_assert(sizeof(std::complex<float>) == sizeof(float) * 2);

    static void fft(std::vector<std::complex<float>>& data) { fft(data.data(), data.size()); }
    static void fft(std::complex<float>* data, size_t size) {
        hirari_fft_utils_process(reinterpret_cast<float*>(data), size, false);
    }
    static void ifft(std::complex<float>* data, size_t size) {
        hirari_fft_utils_process(reinterpret_cast<float*>(data), size, true);
    }
    static void ifft(std::vector<std::complex<float>>& data) { ifft(data.data(), data.size()); }
};

} // namespace Hirari::DSP::Utils
