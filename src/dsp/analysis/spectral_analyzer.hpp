#pragma once
#include "../../core/concurrency/forensic_scratchpad.hpp"
#include "fast_fft.hpp"
#include <algorithm>

namespace Aura::DSP::Analysis {

/**
 * @class SpectralAnalyzer
 * @brief Professional-grade frequency domain profiling engine.
 * INDUSTRIAL: Leverages the ForensicScratchpad for zero-allocation temporary FFT buffers.
 * This ensures that high-resolution visual analysis never impacts the engine's real-time integrity.
 */
class SpectralAnalyzer {
public:
    /**
     * @brief Performs FFT-based spectral analysis on a block of audio.
     */
    void analyze(const float* input, uint32_t size, float* magnitudeOutput) {
        auto& scratch = ::Aura::Core::Concurrency::ForensicScratchpad::getThreadLocal();
        
        // INDUSTRIAL: Zero-allocation scratchpad allocation for FFT workspace.
        // We need 2x size for Real/Imag components (Split-Complex).
        float* real = scratch.allocateArray<float>(size);
        float* imag = scratch.allocateArray<float>(size);

        if (!real || !imag) return; // Sovereign overflow safety

        // 1. WINDOWING & PREPARATION
        for (uint32_t i = 0; i < size; ++i) {
            float window = 0.5f * (1.0f - std::cos(2.0f * M_PI * i / (size - 1)));
            real[i] = input[i] * window;
            imag[i] = 0.0f;
        }

        // 2. INDUSTRIAL FFT EXECUTION
        // Re-using our Split-Complex SIMD optimized FFT logic.
        FastFFT::performFFT(real, imag, size);

        // 3. MAGNITUDE CALCULATION
        for (uint32_t i = 0; i < size / 2; ++i) {
            magnitudeOutput[i] = std::sqrt(real[i] * real[i] + imag[i] * imag[i]);
        }
    }
};

} // namespace Aura::DSP::Analysis
