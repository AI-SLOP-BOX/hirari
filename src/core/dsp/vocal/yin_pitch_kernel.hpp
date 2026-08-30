#pragma once
#include <vector>
#include <cmath>
#include <algorithm>

namespace Aura::DSP::Vocal {

/**
 * @class YinPitchKernel
 * @brief High-precision, time-domain pitch detection kernel.
 * Implements the YIN algorithm with cumulative mean normalized difference.
 */
class YinPitchKernel {
public:
    YinPitchKernel(size_t bufferSize = 2048) : m_bufferSize(bufferSize) {
        m_yinBuffer.resize(bufferSize / 2, 0.0f);
    }

    /**
     * @brief Detects the fundamental frequency (F0) of the input signal.
     * @param signal: Input audio buffer (must be at least bufferSize).
     * @param sampleRate: The sample rate of the signal.
     * @param threshold: The absolute threshold for pitch detection (typically 0.1 - 0.15).
     * @return The detected frequency in Hz, or -1.0 if no pitch detected.
     */
    float detect(const float* signal, double sampleRate, float threshold = 0.15f) {
        size_t halfSize = m_bufferSize / 2;

        // 1. Difference function
        for (size_t tau = 0; tau < halfSize; ++tau) {
            float diff = 0.0f;
            for (size_t i = 0; i < halfSize; ++i) {
                float d = signal[i] - signal[i + tau];
                diff += d * d;
            }
            m_yinBuffer[tau] = diff;
        }

        // 2. Cumulative mean normalized difference function
        m_yinBuffer[0] = 1.0f;
        float runningSum = 0.0f;
        for (size_t tau = 1; tau < halfSize; ++tau) {
            runningSum += m_yinBuffer[tau];
            m_yinBuffer[tau] *= (float)tau / (runningSum + 1e-10f);
        }

        // 3. Absolute thresholding
        size_t tau = 0;
        for (tau = 1; tau < halfSize; ++tau) {
            if (m_yinBuffer[tau] < threshold) {
                while (tau + 1 < halfSize && m_yinBuffer[tau + 1] < m_yinBuffer[tau]) {
                    tau++;
                }
                break;
            }
        }

        if (tau == halfSize || m_yinBuffer[tau] >= threshold) {
            return -1.0f; // No pitch found
        }

        // 4. Parabolic interpolation for sub-sample precision
        float betterTau = (float)tau;
        if (tau > 0 && tau < halfSize - 1) {
            float s0 = m_yinBuffer[tau - 1];
            float s1 = m_yinBuffer[tau];
            float s2 = m_yinBuffer[tau + 1];
            float denom = s2 - 2.0f * s1 + s0;
            if (std::abs(denom) > 1e-6f) {
                betterTau = (float)tau + (s0 - s2) / (2.0f * denom);
            }
        }

        return (float)(sampleRate / betterTau);
    }

private:
    size_t m_bufferSize;
    std::vector<float> m_yinBuffer;
};

} // namespace Aura::DSP::Vocal
