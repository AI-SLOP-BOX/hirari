#pragma once

#include "../../core/rust_ffi.hpp"

#include <cstddef>

namespace Hirari::DSP::Analysis {

/** Rust-owned radix-2 FFT plan exposed to existing DSP callers. */
class FastFFT {
public:
    explicit FastFFT(size_t size) : m_state(hirari_fft_plan_create(size)) {}
    ~FastFFT() { hirari_fft_plan_destroy(m_state); }

    FastFFT(const FastFFT&) = delete;
    FastFFT& operator=(const FastFFT&) = delete;

    bool valid() const noexcept { return hirari_fft_plan_valid(m_state); }

    void forward(float* real, float* imag) const {
        hirari_fft_forward(m_state, real, imag);
    }

    void inverse(float* real, float* imag) const {
        hirari_fft_inverse(m_state, real, imag);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Analysis
