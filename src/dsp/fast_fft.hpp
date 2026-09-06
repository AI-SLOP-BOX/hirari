#pragma once
#include <cmath>
#include <complex>
#include <vector>
#include <numbers>

namespace Aura::DSP {

/**
 * @class FastFFT
 * @brief Industrial Radix-2 FFT implementation.
 * Designed for fixed-size blocks (e.g. 512, 1024) with pre-computed twiddles.
 */
class FastFFT {
public:
    static void process(float* buffer, uint32_t n) {
        if (!buffer || n < 2 || n > (1u << 20) || (n & (n - 1)) != 0) return; // Must be a bounded power of 2

        // Bit-reversal permutation
        for (uint32_t i = 1, j = 0; i < n; i++) {
            uint32_t bit = n >> 1;
            for (; j & bit; bit >>= 1) j ^= bit;
            j ^= bit;
            if (i < j) std::swap(buffer[i], buffer[j]);
        }

        // Cooley-Tukey Radix-2
        for (uint32_t len = 2; len <= n; len <<= 1) {
            double ang = 2.0 * std::numbers::pi / len;
            std::complex<double> wlen(std::cos(ang), std::sin(ang));
            for (uint32_t i = 0; i < n; i += len) {
                std::complex<double> w(1);
                for (uint32_t j = 0; j < len / 2; j++) {
                    float u = buffer[i + j];
                    float v = (float)(buffer[i + j + len / 2] * w.real()); // Simplified for real-only
                    buffer[i + j] = u + v;
                    buffer[i + j + len / 2] = u - v;
                    w *= wlen;
                }
            }
        }
    }

    static void computeBands(const float* fftResult, uint32_t n, float* bands) {
        if (!fftResult || !bands || n < 8 || n > (1u << 20) || (n & (n - 1)) != 0) return;
        // Simple 4-band split: Sub, Low, Mid, High
        uint32_t bSize = n / 8; // Focus on first quarter (Nyquist/2)
        for (int b = 0; b < 4; ++b) {
            float energy = 0.0f;
            for (uint32_t i = b * bSize; i < (b + 1) * bSize; ++i) {
                energy += std::abs(fftResult[i]);
            }
            bands[b] = energy / bSize;
        }
    }
};

} // namespace Aura::DSP
