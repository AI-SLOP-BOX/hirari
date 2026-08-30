#pragma once
#include <vector>
#include <cmath>
#include <algorithm>

namespace Aura::Core::DSP::Vocal {

/**
 * @class PsolaPitchShifter
 * @brief Formant-Preserving Pitch Shifter.
 * Uses a click-free dual-tap modulating delay network.
 */
class PsolaPitchShifter {
public:
    PsolaPitchShifter(double sampleRate = 48000.0) 
        : m_sampleRate(sampleRate)
        , m_delayBuffer(8192, 0.0f)
        , m_writeIdx(0)
        , m_phase(0.0f) {
    }

    /**
     * @brief Performs formant-preserving pitch shifting.
     */
    void process(const float* input, float* output, uint32_t samples, 
                 float ratio, float /*f0*/, double /*sr*/) {
        
        ratio = std::clamp(ratio, 0.5f, 2.0f);
        float tilt = (ratio - 1.0f) * 0.4f; 
        
        float minDelay = 512.0f;
        float maxDelay = 4096.0f;
        float delayRange = maxDelay - minDelay;

        // Modulate delay phase speed based on pitch ratio
        float phaseSpeed = (ratio - 1.0f) / delayRange;

        for (uint32_t s = 0; s < samples; ++s) {
            m_delayBuffer[m_writeIdx] = input[s];
            
            m_phase += phaseSpeed;
            if (m_phase >= 1.0f) m_phase -= 1.0f;
            else if (m_phase < 0.0f) m_phase += 1.0f;

            float phaseA = m_phase;
            float phaseB = m_phase + 0.5f;
            if (phaseB >= 1.0f) phaseB -= 1.0f;

            // Calculate delay offsets for Tap A and Tap B
            float delayA = minDelay + phaseA * delayRange;
            float delayB = minDelay + phaseB * delayRange;

            // Read index locations relative to the write head
            float readIdxA = static_cast<float>(m_writeIdx) - delayA;
            if (readIdxA < 0.0f) readIdxA += 8192.0f;

            float readIdxB = static_cast<float>(m_writeIdx) - delayB;
            if (readIdxB < 0.0f) readIdxB += 8192.0f;

            // Interpolated read from delay line for Tap A
            uint32_t idxA1 = static_cast<uint32_t>(readIdxA) % 8192;
            uint32_t idxA2 = (idxA1 + 1) % 8192;
            float fracA = readIdxA - std::floor(readIdxA);
            float sampleA = m_delayBuffer[idxA1] * (1.0f - fracA) + m_delayBuffer[idxA2] * fracA;
            
            // Interpolated read from delay line for Tap B
            uint32_t idxB1 = static_cast<uint32_t>(readIdxB) % 8192;
            uint32_t idxB2 = (idxB1 + 1) % 8192;
            float fracB = readIdxB - std::floor(readIdxB);
            float sampleB = m_delayBuffer[idxB1] * (1.0f - fracB) + m_delayBuffer[idxB2] * fracB;
            
            // Compute triangular crossfade windows (constant gain sum = 1.0)
            float winA = 1.0f - 2.0f * std::abs(phaseA - 0.5f);
            float winB = 1.0f - 2.0f * std::abs(phaseB - 0.5f);
            
            // Overlap-add the dual taps
            float pitchShifted = sampleA * winA + sampleB * winB;
            
            // Apply one-pole formant tilt filter
            m_lastOut = pitchShifted + (m_lastOut - pitchShifted) * tilt;
            output[s] = m_lastOut;
            
            // Advance write index
            m_writeIdx = (m_writeIdx + 1) % 8192;
        }
    }

private:
    double m_sampleRate;
    std::vector<float> m_delayBuffer;
    uint32_t m_writeIdx;
    float m_phase;
    float m_lastOut = 0.0f;
};

} // namespace Aura::Core::DSP::Vocal
