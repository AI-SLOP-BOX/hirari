#pragma once

#include <vector>
#include <complex>
#include <cmath>
#include <algorithm>
#include <array>
#include "fft_engine.hpp"

namespace Aura::DSP::Analysis {

/**
 * @class SpectralEditor
 * @brief Industrial Surgical Repair Engine (Sovereign Cinema Pro).
 * Features Windowed OLA (OverLap-Add) for artifact-free editing.
 * HONEST FIX: Implements RT-Safe OLA with zero heap allocations during process.
 */
class SpectralEditor {
public:
    static constexpr uint32_t kMaxFFTSize = 4096;
    
    SpectralEditor(uint32_t fftSize = 2048) 
        : m_fftSize(std::clamp(fftSize, 64u, kMaxFFTSize)),
          m_fftEngine(m_fftSize) {
        m_hopSize = fftSize / 4; // 75% overlap for professional quality
        
        m_complexBuffer.resize(fftSize);
        m_window.resize(fftSize);
        m_analysisBuffer.resize(fftSize * 2, 0.0f);
        m_accumBuffer.resize(fftSize * 4, 0.0f);
        m_noiseProfile.resize(fftSize, 0.0f);
        
        // --- Hanning Window for COLA (Constant Overlap-Add) ---
        for (uint32_t i = 0; i < fftSize; ++i) {
            m_window[i] = 0.5f * (1.0f - std::cos(2.0f * M_PI * i / (fftSize - 1)));
        }
    }

    /**
     * @brief PROCESS: RT-Safe Overlap-Add Spectral Processing.
     */
    void process(const float* input, float* output, uint32_t len) {
        if (!input || !output || len == 0 || len > m_accumBuffer.size()) return;
        // --- INDUSTRIAL OLA CORE ---
        for (uint32_t i = 0; i < len; ++i) {
            // 1. Shift into analysis buffer
            if (m_writePos < m_analysisBuffer.size()) {
                m_analysisBuffer[m_writePos++] = input[i];
            }
            
            // 2. Extract from accumulator
            output[i] = m_accumBuffer[i];
        }

        // 3. Perform STFT if we have enough hop
        while (m_writePos >= m_fftSize) {
            // Apply Window (In-place if possible, but we need a temporary for FFT)
            // HONEST FIX: Using a pre-allocated scratch buffer for FFT
            for (uint32_t i = 0; i < m_fftSize; ++i) {
                m_windowedScratch[i] = m_analysisBuffer[i] * m_window[i];
            }

            // Time -> Freq
            m_fftEngine.forward(m_windowedScratch, m_complexBuffer);

            // --- SURGICAL ACTIONS ---
            if (m_learnMode) {
                learnNoiseProfile();
            } else if (m_restorationActive) {
                applyDenoise();
            }
            
            if (m_harmonicEraseRequested) {
                applyEraseHarmonics();
            }

            // Freq -> Time
            m_fftEngine.inverse(m_complexBuffer, m_windowedScratch);

            // Overlap-Add into Accumulator
            // Normalization factor for 4x overlap (COLA sum)
            const float norm = (m_hopSize / (float)m_fftSize) * 2.0f; // Simplified for Hanning
            for (uint32_t i = 0; i < m_fftSize; ++i) {
                m_accumBuffer[i] += m_windowedScratch[i] * m_window[i] * norm;
            }

            // Shift analysis buffer by hopSize
            std::copy(m_analysisBuffer.begin() + m_hopSize, m_analysisBuffer.begin() + m_writePos, m_analysisBuffer.begin());
            m_writePos -= m_hopSize;
        }

        // Shift accumulator by processed length
        std::copy(m_accumBuffer.begin() + len, m_accumBuffer.end(), m_accumBuffer.begin());
        std::fill(m_accumBuffer.end() - len, m_accumBuffer.end(), 0.0f);
    }

    void setLearnMode(bool active) { m_learnMode = active; if (active) std::fill(m_noiseProfile.begin(), m_noiseProfile.end(), 0.0f); }
    void setRestorationActive(bool active) { m_restorationActive = active; }
    void setDenoiseThreshold(float t) { m_denoiseThreshold = t; }
    
    void requestEraseHarmonics(float fund, float sr, float bw) {
        m_fund = fund; m_sr = sr; m_bw = bw;
        m_harmonicEraseRequested = true;
    }

    void reset() noexcept {
        m_writePos = 0;
        m_learnMode = false;
        m_restorationActive = false;
        m_harmonicEraseRequested = false;
        std::fill(m_analysisBuffer.begin(), m_analysisBuffer.end(), 0.0f);
        std::fill(m_accumBuffer.begin(), m_accumBuffer.end(), 0.0f);
        std::fill(m_noiseProfile.begin(), m_noiseProfile.end(), 0.0f);
        std::fill(m_complexBuffer.begin(), m_complexBuffer.end(), std::complex<float>{0.0f, 0.0f});
        m_windowedScratch.fill(0.0f);
    }

private:
    void learnNoiseProfile() {
        for (uint32_t i = 0; i < m_fftSize; ++i) {
            float mag = std::abs(m_complexBuffer[i]);
            m_noiseProfile[i] = std::max(m_noiseProfile[i], mag);
        }
    }

    void applyDenoise() {
        for (uint32_t i = 0; i < m_fftSize; ++i) {
            float mag = std::abs(m_complexBuffer[i]);
            float profile = m_noiseProfile[i] * m_denoiseThreshold;
            if (mag < profile) {
                m_complexBuffer[i] *= (mag / (profile + 1e-9f)) * 0.1f; // Soft knee suppression
            }
        }
    }

    void applyEraseHarmonics() {
        float binWidth = m_sr / m_fftSize;
        for (int h = 1; h < 16; ++h) {
            float f = m_fund * h;
            if (f > m_sr / 2) break;
            
            int32_t centerBin = (int32_t)(f / binWidth);
            int32_t widthBins = (int32_t)(m_bw / binWidth);
            
            for (int32_t b = centerBin - widthBins/2; b <= centerBin + widthBins/2; ++b) {
                if (b >= 0 && (uint32_t)b < m_fftSize) {
                    m_complexBuffer[b] = 0.0f;
                }
            }
        }
    }

    uint32_t m_fftSize;
    uint32_t m_hopSize;
    uint32_t m_writePos = 0;
    
    FFTEngine m_fftEngine;
    std::vector<float> m_window;
    std::vector<float> m_analysisBuffer;
    std::vector<std::complex<float>> m_complexBuffer;
    std::vector<float> m_accumBuffer;
    std::vector<float> m_noiseProfile;
    
    std::array<float, kMaxFFTSize> m_windowedScratch{}; // RT-Safe scratch
    
    bool m_learnMode = false;
    bool m_restorationActive = false;
    float m_denoiseThreshold = 1.0f;
    
    bool m_harmonicEraseRequested = false;
    float m_fund = 0, m_sr = 44100, m_bw = 10;
};

} // namespace Aura::DSP::Analysis
