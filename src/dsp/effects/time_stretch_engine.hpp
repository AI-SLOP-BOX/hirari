#pragma once

#include <vector>
#include <cmath>
#include <algorithm>

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

namespace Aura::DSP::Effects {

/**
 * @class TimeStretchEngine
 * @brief Professional Logic Pro-style "Flex Time" Pitch-Preserving stretching.
 * Powered by Overlap-Add (OLA) grain resynthesis with pre-allocated circular histories
 * to stretch duration without altering pitch, running with zero dynamic heap allocations.
 */
class TimeStretchEngine {
public:
    TimeStretchEngine(double sr = 44100.0) 
        : m_sampleRate(sr)
        , m_pAnalysis(0.0)
        , m_pSynthesis(0)
        , m_playhead(0)
        , m_inputHistoryPos(0) {
        m_overlapBuffer.assign(16384, 0.0f);
        m_inputHistory.assign(65536, 0.0f);
        m_window.resize(2048);
        for (uint32_t i = 0; i < 2048; ++i) {
            m_window[i] = 0.5f * (1.0f - std::cos(2.0f * M_PI * i / 2047.0f));
        }
        if (!std::isfinite(m_sampleRate) || m_sampleRate < 8000.0 || m_sampleRate > 384000.0)
            m_sampleRate = 44100.0;
    }

    /**
     * @brief Stretches a block of audio without altering pitch.
     * @param ratio: 1.0 = Normal, 2.0 = Half speed, 0.5 = Double speed.
     */
    void process(const float* input, float* output, uint32_t numSamples, float ratio) {
        if (!input || !output || numSamples == 0) return;
        if (!std::isfinite(ratio) || ratio <= 0.0f) ratio = 1.0f;
        if (std::abs(ratio - 1.0f) < 0.01f || ratio < 0.2f || ratio > 5.0f) {
            std::copy(input, input + numSamples, output);
            return;
        }

        // 1. Load input block into continuous input history buffer
        for (uint32_t i = 0; i < numSamples; ++i) {
            const float value = std::isfinite(input[i]) ? std::clamp(input[i], -16.0f, 16.0f) : 0.0f;
            m_inputHistory[(m_inputHistoryPos + i) % m_inputHistory.size()] = value;
        }

        std::fill(output, output + numSamples, 0.0f);

        uint32_t N = 2048;
        uint32_t Hs = 512;
        double Ha = Hs * ratio;

        // 2. Perform OLA grain synthesis relative to absolute continuous playhead
        for (uint32_t outIdx = 0; outIdx < numSamples; ++outIdx) {
            uint64_t absOutPos = m_playhead + outIdx;
            
            while (m_pSynthesis <= absOutPos) {
                // Synthesize a grain and overlap-add directly into the circular buffer
                for (uint32_t i = 0; i < N; ++i) {
                    double readPos = m_pAnalysis + i;
                    uint64_t i0 = static_cast<uint64_t>(readPos);
                    uint64_t i1 = i0 + 1;
                    float frac = static_cast<float>(readPos - i0);
                    
                    // Retrieve interpolated samples from the continuous input history buffer
                    float s0 = m_inputHistory[i0 % m_inputHistory.size()];
                    float s1 = m_inputHistory[i1 % m_inputHistory.size()];
                    float sample = s0 * (1.0f - frac) + s1 * frac;
                    
                    uint64_t writePos = (m_pSynthesis + i) % m_overlapBuffer.size();
                    m_overlapBuffer[writePos] += sample * m_window[i];
                }

                m_pAnalysis += Ha;
                m_pSynthesis += Hs;
            }

            // Extract the final overlap-added sample at the current playhead position
            uint64_t readPos = absOutPos % m_overlapBuffer.size();
            output[outIdx] = std::isfinite(m_overlapBuffer[readPos])
                ? m_overlapBuffer[readPos] * 0.5f : 0.0f; // overlap normalization
            m_overlapBuffer[readPos] = 0.0f; // Clear buffer for subsequent cycles
        }

        m_inputHistoryPos += numSamples;
        m_playhead += numSamples;
    }

    void reset() {
        m_pAnalysis = 0.0;
        m_pSynthesis = 0;
        m_playhead = 0;
        m_inputHistoryPos = 0;
        std::fill(m_overlapBuffer.begin(), m_overlapBuffer.end(), 0.0f);
        std::fill(m_inputHistory.begin(), m_inputHistory.end(), 0.0f);
    }

private:
    double m_sampleRate;
    double m_pAnalysis;
    uint64_t m_pSynthesis;
    uint64_t m_playhead;
    uint64_t m_inputHistoryPos;
    
    std::vector<float> m_window;
    std::vector<float> m_overlapBuffer;
    std::vector<float> m_inputHistory;
};

} // namespace Aura::DSP::Effects
