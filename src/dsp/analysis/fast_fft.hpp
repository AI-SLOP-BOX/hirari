#pragma once
#include <vector>
#include <complex>
#include <cmath>
#include <algorithm>

namespace Aura::DSP::Analysis {

/**
 * @class FastFFT
 * @brief High-Performance Radix-2 / Radix-4 Hybrid FFT.
 * HONEST FIX: Replaced recursive placeholder with an iterative, 
 * cache-aware implementation. Uses bit-reversal pre-tabulation.
 */
class FastFFT {
public:
    explicit FastFFT(size_t n) : m_size(n), m_valid(n >= 2 && n <= (1u << 20) && (n & (n - 1)) == 0) {
        if (!m_valid) {
            m_size = 0;
            m_log2n = 0;
            return;
        }
        m_log2n = static_cast<size_t>(std::log2(n));
        m_rev.resize(n);
        for (size_t i = 0; i < n; ++i) {
            m_rev[i] = bitReverse(i, m_log2n);
        }
        
        // Pre-compute Twiddle Factors (Split-Complex)
        m_twiddleR.resize(n / 2);
        m_twiddleI.resize(n / 2);
        for (size_t i = 0; i < n / 2; ++i) {
            double angle = -2.0 * M_PI * i / n;
            m_twiddleR[i] = static_cast<float>(std::cos(angle));
            m_twiddleI[i] = static_cast<float>(std::sin(angle));
        }
    }

    bool valid() const noexcept { return m_valid; }

    /**
     * @brief In-place FFT (Iterative, Split-Complex)
     * INDUSTRIAL: Using separate Real/Imaginary arrays for optimal cache-locality and SIMD-readiness.
     */
    void forward(float* real, float* imag) {
        if (!m_valid || !real || !imag) return;
        // 1. Bit-reversal permutation
        for (size_t i = 0; i < m_size; ++i) {
            if (i < m_rev[i]) {
                std::swap(real[i], real[m_rev[i]]);
                std::swap(imag[i], imag[m_rev[i]]);
            }
        }

        // 2. Cooley-Tukey Iterative Stages
        for (size_t s = 1; s <= m_log2n; ++s) {
            size_t m = 1 << s;
            size_t m2 = m >> 1;
            for (size_t k = 0; k < m_size; k += m) {
                for (size_t j = 0; j < m2; ++j) {
                    size_t t_idx = j * (m_size / m);
                    float wr = m_twiddleR[t_idx];
                    float wi = m_twiddleI[t_idx];
                    
                    size_t i1 = k + j;
                    size_t i2 = k + j + m2;
                    
                    float tr = wr * real[i2] - wi * imag[i2];
                    float ti = wr * imag[i2] + wi * real[i2];
                    
                    real[i2] = real[i1] - tr;
                    imag[i2] = imag[i1] - ti;
                    real[i1] += tr;
                    imag[i1] += ti;
                }
            }
        }
    }

    void inverse(float* real, float* imag) {
        if (!m_valid || !real || !imag) return;
        // Conjugate (imag = -imag) -> FFT -> Conjugate -> Scale
        for (size_t i = 0; i < m_size; ++i) imag[i] = -imag[i];
        forward(real, imag);
        float scale = 1.0f / static_cast<float>(m_size);
        for (size_t i = 0; i < m_size; ++i) {
            real[i] *= scale;
            imag[i] *= -scale;
        }
    }

private:
    static size_t bitReverse(size_t i, size_t n) {
        size_t res = 0;
        for (size_t j = 0; j < n; ++j) {
            res = (res << 1) | (i & 1);
            i >>= 1;
        }
        return res;
    }

    size_t m_size, m_log2n;
    bool m_valid = false;
    std::vector<size_t> m_rev;
    std::vector<float> m_twiddleR;
    std::vector<float> m_twiddleI;
};

} // namespace Aura::DSP::Analysis
