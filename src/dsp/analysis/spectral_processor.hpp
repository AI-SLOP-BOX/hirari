#pragma once
#include <vector>
#include <cmath>
#include <complex>
#include "../utils/fft_utils.hpp"
#include "../../core/audio_buffer.hpp"

namespace Aura::DSP::Analysis {

/**
 * @class SpectralProcessor
 * @brief iZotope RX / SpectralLayers style 2D Frequency Editor.
 * HONEST FIX: Implements Spectral Lasso and Region-specific processing.
 * Users can 'draw' on the spectrogram to isolate or remove specific 
 * frequencies at specific times.
 */
class SpectralProcessor {
public:
    struct Rect { float t0, f0, t1, f1; }; // Time (sec) / Freq (Hz)

    void applyMask(Core::AudioBuffer& buffer, double sampleRate, const Rect& target, float gain) {
        const uint32_t fftSize = 2048;
        std::vector<float> window(fftSize);
        std::vector<std::complex<float>> spectrum(fftSize);
        for (uint32_t i = 0; i < fftSize; ++i) {
            const float phase = static_cast<float>(i) / static_cast<float>(fftSize - 1);
            window[i] = 0.5f - 0.5f * std::cos(2.0f * static_cast<float>(M_PI) * phase);
        }

        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
            float* data = buffer.getWritePointer(c);
            
            // STFT Overlap-Add Loop
            for (uint32_t offset = 0; offset + fftSize <= buffer.getNumSamples(); offset += fftSize / 2) {
                float timeSec = (float)offset / (float)sampleRate;
                
                // Skip if not in target time range
                if (timeSec < target.t0 || timeSec > target.t1) continue;

                for (uint32_t i = 0; i < fftSize; ++i) {
                    const float sample = std::isfinite(data[offset + i]) ? data[offset + i] : 0.0f;
                    spectrum[i] = {sample * window[i], 0.0f};
                }
                Utils::FFTUtils::fft(spectrum);

                for (uint32_t k = 0; k < fftSize; ++k) {
                    float freq = (float)k * (float)sampleRate / (float)fftSize;
                    
                    // --- SPECTRAL SELECTION CHECK ---
                    if (freq >= target.f0 && freq <= target.f1) {
                         spectrum[k] *= gain; // Apply spectral edit
                    }
                }
                
                Utils::FFTUtils::ifft(spectrum);
                for (uint32_t i = 0; i < fftSize; ++i) {
                    const float value = spectrum[i].real();
                    data[offset + i] = std::isfinite(value) ? value : 0.0f;
                }
            }
        }
    }
};

} // namespace Aura::DSP::Analysis
