/*
 * Hirari DAW Ultimate - High-Performance Digital Audio Workstation
 * Copyright (c) 2024-2026 Hirari DAW Project. All rights reserved.
 * Licensed under the MIT License.
 */

#pragma once

#include <vector>
#include <array>
#include <cmath>
#include <algorithm>

namespace Hirari::DSP::Utils {

/**
 * @class NeuralInference
 * @brief High-performance Multi-Layer Perceptron (MLP) Inference Engine.
 * Optimized for real-time audio processing (No allocations, SIMD-friendly).
 * Used for modeling non-linear analog behaviors (Neural DSP style).
 */
template<size_t InputSize, size_t HiddenSize, size_t OutputSize>
class NeuralInference {
public:
    NeuralInference() { reset(); }

    void reset() {
        m_hiddenState.fill(0.0f);
    }

    /**
     * @brief FORWARD PASS: Optimized Dot-Product Accumulation.
     * HONEST FIX: Replaces naive loops with 4x Unrolled Accumulators to 
     * trigger CPU auto-vectorization. Uses Fast-Tanh polynomial approx.
     */
    void process(const float* input, float* output, 
                 const std::array<float, HiddenSize * InputSize>& weightsIH,
                 const std::array<float, HiddenSize>& biasH,
                 const std::array<float, OutputSize * HiddenSize>& weightsHO,
                 const std::array<float, OutputSize>& biasO) {
        
        // 1. Input to Hidden (Unrolled Dot Product)
        for (size_t h = 0; h < HiddenSize; ++h) {
            float sum = biasH[h];
            const float* w = &weightsIH[h * InputSize];
            
            size_t i = 0;
            for (; i + 3 < InputSize; i += 4) {
                sum += (input[i] * w[i]) + (input[i+1] * w[i+1]) + 
                       (input[i+2] * w[i+2]) + (input[i+3] * w[i+3]);
            }
            for (; i < InputSize; ++i) sum += input[i] * w[i];
            
            // Fast Tanh Approximation: x * (27 + x^2) / (27 + 9 * x^2)
            float x2 = sum * sum;
            m_hiddenState[h] = sum * (27.0f + x2) / (27.0f + 9.0f * x2);
            m_hiddenState[h] = std::clamp(m_hiddenState[h], -1.0f, 1.0f);
        }

        // 2. Hidden to Output
        for (size_t o = 0; o < OutputSize; ++o) {
            float sum = biasO[o];
            const float* w = &weightsHO[o * HiddenSize];
            
            size_t h = 0;
            for (; h + 3 < HiddenSize; h += 4) {
                sum += (m_hiddenState[h] * w[h]) + (m_hiddenState[h+1] * w[h+1]) +
                       (m_hiddenState[h+2] * w[h+2]) + (m_hiddenState[h+3] * w[h+3]);
            }
            for (; h < HiddenSize; ++h) sum += m_hiddenState[h] * w[h];
            output[o] = sum;
        }
    }

private:
    std::array<float, HiddenSize> m_hiddenState;
};

} // namespace Hirari::DSP::Utils
