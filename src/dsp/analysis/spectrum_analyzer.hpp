#pragma once
#include "fast_fft.hpp"
#include <algorithm>
#include <array>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include <vector>
#include <complex>

namespace Aura::DSP::Analysis {

class SpectrumAnalyzer {
public:
    static constexpr uint32_t kFFTSize = 1024;
    static constexpr uint8_t kNumBands = 64;

    SpectrumAnalyzer([[maybe_unused]] double sr = 44100.0) : m_fft(kFFTSize) {
        m_win.resize(kFFTSize);
        m_realBuffer.resize(kFFTSize);
        m_imagBuffer.resize(kFFTSize);
        for (uint32_t i = 0; i < kFFTSize; ++i) {
             m_win[i] = 0.5f * (1.0f - std::cos(2.0f * M_PI * i / (kFFTSize - 1)));
        }
        for (auto& b : m_bands) b.store(0.0f);
    }

    void process(const float* data, size_t size, double sr) {
        if (!data || size < kFFTSize || !std::isfinite(sr) || sr < 1.0 || sr > 768000.0) return;

        // 1. WINDOWED PREP
        for (size_t i = 0; i < kFFTSize; ++i) {
            const float sample = std::isfinite(data[i]) ? data[i] : 0.0f;
            m_realBuffer[i] = sample * m_win[i];
            m_imagBuffer[i] = 0.0f;
        }
        
        m_fft.forward(m_realBuffer.data(), m_imagBuffer.data());

        // 2. LOG BAND MAPPING
        std::array<float, kNumBands> newBands;
        newBands.fill(0.0f);
        
        for (uint32_t b = 0; b < kFFTSize / 2; ++b) {
            float r = m_realBuffer[b];
            float im = m_imagBuffer[b];
            float mag = std::sqrt(r * r + im * im) / kFFTSize;
            if (!std::isfinite(mag)) mag = 0.0f;
            float freq = (float)b * (float)sr / (float)kFFTSize;
            
            if (freq > 20.0f) {
                int idx = static_cast<int>(kNumBands * (std::log10(freq / 20.0f) / std::log10(20000.0f / 20.0f)));
                if (idx >= 0 && idx < kNumBands) {
                    newBands[idx] = std::max(newBands[idx], mag);
                }
            }
        }

        // 3. BALLISTICS
        for (int i = 0; i < kNumBands; ++i) {
            float prev = m_bands[i].load(std::memory_order_relaxed);
            float target = newBands[i];
            float decay = (target > prev) ? 0.3f : 0.05f;
            m_bands[i].store(prev + (target - prev) * decay, std::memory_order_relaxed);
        }
    }

    std::vector<float> getCurrentBands() const {
        std::vector<float> out(kNumBands);
        for (int i = 0; i < kNumBands; ++i) out[i] = m_bands[i].load(std::memory_order_relaxed);
        return out;
    }

private:
    FastFFT m_fft;
    std::vector<float> m_win;
    std::vector<float> m_realBuffer;
    std::vector<float> m_imagBuffer;
    std::array<std::atomic<float>, kNumBands> m_bands;
};

} // namespace Aura::DSP::Analysis
