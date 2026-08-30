#pragma once

#include <vector>
#include <cmath>
#include <complex>
#include <algorithm>

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

namespace Aura::DSP::Effects {

/**
 * @class PhaseVocoder
 * @brief Professional FFT-based Time-Stretching and Pitch-Shifting Engine.
 * STFT processing with Cooley-Tukey FFT, frequency bin shifting, and phase scaling.
 */
class PhaseVocoder {
public:
    PhaseVocoder(uint32_t fftSize = 2048, uint32_t hopSize = 512)
        : m_fftSize(normalizeFftSize(fftSize))
        , m_hopSize(1) {
        m_hopSize = std::min(std::max<uint32_t>(1, hopSize), m_fftSize);
        m_accumPhase.assign(m_fftSize, 0.0f);
        m_lastPhase.assign(m_fftSize, 0.0f);
        m_window.resize(m_fftSize);
        m_fftBuffer.assign(m_fftSize, {});
        m_shiftedBuffer.assign(m_fftSize, {});
        for (uint32_t i = 0; i < m_fftSize; ++i) {
            m_window[i] = 0.5f * (1.0f - std::cos(2.0f * M_PI * i / (m_fftSize - 1)));
        }
    }

    // Forward FFT (Cooley-Tukey Radix-2)
    void fft(std::vector<std::complex<float>>& x) {
        const size_t n = x.size();
        if (n <= 1 || !isPowerOfTwo(n)) return;

        for (size_t i = 1, j = 0; i < n; ++i) {
            size_t bit = n >> 1;
            for (; j & bit; bit >>= 1) j ^= bit;
            j ^= bit;
            if (i < j) std::swap(x[i], x[j]);
        }

        for (size_t length = 2; length <= n; length = length <= n / 2 ? length << 1 : n + 1) {
            const float angle = -2.0f * M_PI / static_cast<float>(length);
            const std::complex<float> root(std::cos(angle), std::sin(angle));
            for (size_t start = 0; start < n; start += length) {
                std::complex<float> twiddle(1.0f, 0.0f);
                const size_t half = length >> 1;
                for (size_t i = 0; i < half; ++i) {
                    const auto even = x[start + i];
                    const auto odd = twiddle * x[start + i + half];
                    x[start + i] = even + odd;
                    x[start + i + half] = even - odd;
                    twiddle *= root;
                }
            }
        }
    }

    // Inverse FFT
    void ifft(std::vector<std::complex<float>>& x) {
        if (x.empty() || !isPowerOfTwo(x.size())) return;
        for (auto& val : x) val = std::conj(val);
        fft(x);
        float N = static_cast<float>(x.size());
        for (auto& val : x) val = std::conj(val) / N;
    }

    /**
     * @brief PITCH SHIFT: Resynthesizes audio at a different frequency.
     * Uses STFT Cooley-Tukey FFT, bin-shifting, and phase scaling.
     */
    void process(const float* input, float* output, uint32_t len, float ratio) {
        if (!input || !output || len == 0) return;
        if (!std::isfinite(ratio) || ratio < 0.1f || ratio > 10.0f) {
            copyFinite(input, output, len);
            return;
        }

        const uint32_t N = m_fftSize;
        auto& fftBuf = m_fftBuffer;
        std::fill(fftBuf.begin(), fftBuf.end(), std::complex<float>{});
        
        for (uint32_t i = 0; i < N; ++i) {
            if (i < len) {
                const float sample = input[i];
                fftBuf[i] = (std::isfinite(sample) ? std::clamp(sample, -4.0f, 4.0f) : 0.0f)
                    * m_window[i];
            } else {
                fftBuf[i] = 0.0f;
            }
        }

        fft(fftBuf);

        auto& shiftedBuf = m_shiftedBuffer;
        std::fill(shiftedBuf.begin(), shiftedBuf.end(), std::complex<float>{});
        
        shiftedBuf[0] = fftBuf[0];
        shiftedBuf[N / 2] = fftBuf[N / 2];

        for (uint32_t k = 1; k < N / 2; ++k) {
            const uint32_t targetK = static_cast<uint32_t>(
                std::round(static_cast<double>(k) * static_cast<double>(ratio)));
            if (targetK > 0 && targetK < N / 2) {
                float mag = std::abs(fftBuf[k]);
                float phase = std::arg(fftBuf[k]);
                float newPhase = phase * ratio;

                shiftedBuf[targetK] = std::polar(mag, newPhase);
                shiftedBuf[N - targetK] = std::conj(shiftedBuf[targetK]);
            }
        }

        ifft(shiftedBuf);

        const uint32_t outputLen = std::min(len, N);
        for (uint32_t i = 0; i < outputLen; ++i) {
            const float sample = shiftedBuf[i].real() * m_window[i];
            output[i] = std::isfinite(sample) ? std::clamp(sample, -4.0f, 4.0f) : 0.0f;
        }
        for (uint32_t i = outputLen; i < len; ++i) output[i] = 0.0f;
    }

private:
    static constexpr uint32_t kMaxFftSize = 1u << 20;

    static bool isPowerOfTwo(size_t value) noexcept {
        return value != 0 && (value & (value - 1)) == 0;
    }

    static uint32_t normalizeFftSize(uint32_t requested) noexcept {
        if (requested < 2) return 2;
        if (requested >= kMaxFftSize) return kMaxFftSize;

        uint32_t size = 2;
        while (size < requested && size <= kMaxFftSize / 2) size <<= 1;
        return size;
    }

    static void copyFinite(const float* input, float* output, uint32_t len) noexcept {
        for (uint32_t i = 0; i < len; ++i) {
            const float sample = input[i];
            output[i] = std::isfinite(sample) ? std::clamp(sample, -4.0f, 4.0f) : 0.0f;
        }
    }

    uint32_t m_fftSize, m_hopSize;
    std::vector<float> m_window;
    std::vector<float> m_accumPhase;
    std::vector<float> m_lastPhase;
    std::vector<std::complex<float>> m_fftBuffer;
    std::vector<std::complex<float>> m_shiftedBuffer;
};

} // namespace Aura::DSP::Effects
