#pragma once
#include <cmath>
#include <random>
#include <array>
#include <cstdint>

#if defined(__arm64__) || defined(__aarch64__)
#include <arm_neon.h>
#elif defined(__x86_64__) || defined(_M_X64)
#include <immintrin.h>
#endif

namespace Aura::DSP::Utils {

/**
 * @class TPDFDither
 * @brief High-precision Triangular Probability Density Function Dither.
 * Optimized with Xorshift32 for real-time audio threads.
 */
class TPDFDither {
public:
    TPDFDither() {
        std::random_device rd;
        m_state = rd();
        if (m_state == 0) m_state = 0x12345678;
    }

    /**
     * @brief Generate 1 LSB of TPDF noise at 24-bit level.
     */
    inline float process() {
        uint32_t r1 = xorshift32();
        uint32_t r2 = xorshift32();
        // Summing two uniform dists gives TPDF.
        // Scale to [-1, 1] then to 24-bit LSB.
        float n = (static_cast<float>(r1) * kScale + static_cast<float>(r2) * kScale - 1.0f);
        return n * (1.0f / 8388608.0f);
    }

    void processBlock(float* buffer, uint32_t numSamples) {
        for (uint32_t i = 0; i < numSamples; ++i) {
            buffer[i] += process();
        }
    }

private:
    inline uint32_t xorshift32() {
        m_state ^= m_state << 13;
        m_state ^= m_state >> 17;
        m_state ^= m_state << 5;
        return m_state;
    }

    uint32_t m_state;
    static constexpr float kScale = 1.0f / 4294967295.0f;
};

/**
 * @class NoiseShapingDither
 * @brief Mastering-Grade Psychoacoustic Noise-Shaping Dither.
 * Optimized for high-throughput block processing.
 */
class NoiseShapingDither {
public:
    NoiseShapingDither() {
        m_state = 0x12345678;
        m_errorHistory.fill(0.0f);
    }

    inline float process(float sample, int bits = 16) {
        float bitStep = 1.0f / static_cast<float>(1 << (bits - 1));
        
        uint32_t r1 = xorshift32();
        uint32_t r2 = xorshift32();
        float noise = (static_cast<float>(r1) * kScale + static_cast<float>(r2) * kScale - 1.0f) * bitStep;
        
        float filteredError = m_errorHistory[0] * 2.033f 
                            - m_errorHistory[1] * 2.165f 
                            + m_errorHistory[2] * 1.259f 
                            - m_errorHistory[3] * 0.304f;
                            
        float input = sample + filteredError + noise;
        
        // Fast rounding
        float quantized = std::floor(input / bitStep + 0.5f) * bitStep;
        
        m_errorHistory[3] = m_errorHistory[2];
        m_errorHistory[2] = m_errorHistory[1];
        m_errorHistory[1] = m_errorHistory[0];
        m_errorHistory[0] = input - quantized;

        return quantized;
    }

    void processBlock(float* buffer, uint32_t numSamples, int bits = 16) {
        float bitStep = 1.0f / static_cast<float>(1 << (bits - 1));
        float invBitStep = static_cast<float>(1 << (bits - 1));

        for (uint32_t i = 0; i < numSamples; ++i) {
            uint32_t r1 = xorshift32();
            uint32_t r2 = xorshift32();
            float noise = (static_cast<float>(r1) * kScale + static_cast<float>(r2) * kScale - 1.0f) * bitStep;

            float filteredError = m_errorHistory[0] * 2.033f 
                                - m_errorHistory[1] * 2.165f 
                                + m_errorHistory[2] * 1.259f 
                                - m_errorHistory[3] * 0.304f;
                                
            float input = buffer[i] + filteredError + noise;
            float quantized = std::floor(input * invBitStep + 0.5f) * bitStep;
            
            m_errorHistory[3] = m_errorHistory[2];
            m_errorHistory[2] = m_errorHistory[1];
            m_errorHistory[1] = m_errorHistory[0];
            m_errorHistory[0] = input - quantized;
            
            buffer[i] = quantized;
        }
    }

private:
    inline uint32_t xorshift32() {
        m_state ^= m_state << 13;
        m_state ^= m_state >> 17;
        m_state ^= m_state << 5;
        return m_state;
    }

    uint32_t m_state;
    std::array<float, 4> m_errorHistory{0.0f, 0.0f, 0.0f, 0.0f};
    static constexpr float kScale = 1.0f / 4294967295.0f;
};

} // namespace Aura::DSP::Utils
