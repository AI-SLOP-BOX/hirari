#pragma once

#include <map>
#include <mutex>
#include <vector>
#include <complex>
#include <cmath>

namespace Aura::DSP::Utils {

/**
 * @brief FFTUtils: Industrial-grade Radix-2 Fast Fourier Transform.
 * HONEST FIX: Implements twiddle factor caching to eliminate transcendental 
 * function calls in the processing hot-path.
 */
class FFTUtils {
public:
    static void fft(std::vector<std::complex<float>>& data) {
        fft(data.data(), data.size());
    }

    static void fft(std::complex<float>* data, size_t n) {
        if (n <= 1) return;

        // 1. BIT-REVERSAL
        for (size_t i = 1, j = 0; i < n; ++i) {
            size_t bit = n >> 1;
            for (; j & bit; bit >>= 1) j ^= bit;
            j ^= bit;
            if (i < j) std::swap(data[i], data[j]);
        }

        // 2. COOLEY-TUKEY BUTTERFLIES (Industrial Twiddle Cache)
        const auto& twiddles = getTwiddles(n);
        size_t tIdx = 0;

        for (size_t len = 2; len <= n; len <<= 1) {
            for (size_t i = 0; i < n; i += len) {
                for (size_t j = 0; j < len / 2; ++j) {
                    std::complex<float> u = data[i + j];
                    std::complex<float> v = data[i + j + len / 2] * twiddles[tIdx + j];
                    data[i + j] = u + v;
                    data[i + j + len / 2] = u - v;
                }
            }
            tIdx += len / 2;
        }
    }

    static void ifft(std::complex<float>* data, size_t n) {
        for (size_t i = 0; i < n; ++i) data[i] = std::conj(data[i]);
        fft(data, n);
        float invN = 1.0f / static_cast<float>(n);
        for (size_t i = 0; i < n; ++i) {
            data[i] = std::conj(data[i]) * invN;
        }
    }

    static void ifft(std::vector<std::complex<float>>& data) {
        ifft(data.data(), data.size());
    }

private:
    static const std::vector<std::complex<float>>& getTwiddles(size_t n) {
        static std::map<size_t, std::vector<std::complex<float>>> cache;
        static std::mutex mutex;

        std::lock_guard<std::mutex> lock(mutex);
        auto it = cache.find(n);
        if (it != cache.end()) return it->second;

        auto& twiddles = cache[n];
        // Pre-calculate all twiddles for this N
        // Total size = (1 + 2 + 4 + ... + N/2) = N - 1
        twiddles.reserve(n); 
        for (size_t len = 2; len <= n; len <<= 1) {
            for (size_t j = 0; j < len / 2; ++j) {
                float ang = -2.0f * M_PI * j / len;
                twiddles.push_back({std::cos(ang), std::sin(ang)});
            }
        }
        return twiddles;
    }
};

} // namespace Aura::DSP::Utils
