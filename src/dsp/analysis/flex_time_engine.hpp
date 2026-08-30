#pragma once

#include <vector>
#include <cmath>
#include <algorithm>

namespace Aura::DSP::Analysis {

/**
 * @brief FlexTimeEngine: Professional granular time-stretching.
 * Logic Pro-style "Flex Time" that changes length without affecting pitch.
 * Uses a basic Overlap-Add (OLA) strategy for real-time safety.
 */
class FlexTimeEngine {
public:
    explicit FlexTimeEngine(double sr) : m_sampleRate(sr) {
        // Pre-calculate Hann window
        m_window.resize(kGrainSize);
        for (size_t i = 0; i < kGrainSize; ++i) {
            m_window[i] = 0.5f * (1.0f - std::cos(2.0f * M_PI * i / (kGrainSize - 1)));
        }
    }

    /**
     * @brief Stretches a source buffer into a target buffer.
     * HARDENED: Zero-allocation, pre-windowed OLA.
     */
    void process(const float* source, size_t sourceSize, float* output, size_t targetSize, float factor) {
        if (std::abs(factor - 1.0f) < 0.01f) {
            std::copy(source, source + std::min(sourceSize, targetSize), output);
            return;
        }

        size_t readPos = 0;
        size_t writePos = 0;

        while (readPos + kGrainSize < sourceSize && writePos + kGrainSize < targetSize) {
            for (size_t i = 0; i < kGrainSize; ++i) {
                output[writePos + i] += source[readPos + i] * m_window[i];
            }
            
            readPos += static_cast<size_t>(kHopSize / factor);
            writePos += kHopSize;
        }
    }

private:
    static constexpr size_t kGrainSize = 1024;
    static constexpr size_t kHopSize = 512;
    double m_sampleRate;
    std::vector<float> m_window;
};

} // namespace Aura::DSP::Analysis
