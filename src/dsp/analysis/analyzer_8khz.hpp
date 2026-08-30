#pragma once

#include <atomic>
#include <cmath>
#include <algorithm>

namespace Aura::DSP::Analysis {

/**
 * @class Analyzer8kHz
 * @brief Professional High-Frequency Energy Tracker.
 */
class Analyzer8kHz {
public:
    Analyzer8kHz(double sr = 44100.0);

    void setSampleRate(double sr);

    /**
     * @brief ACCELERATED BPF ANALYSIS.
     */
    void analyze(const float* buffer, size_t numFrames);

    float getEnergy() const;

private:
    std::atomic<float> m_highFreqEnergy{0.0f};
    std::atomic<float> m_rms{0.0f};
    std::atomic<float> m_peak{0.0f};
    double m_sampleRate = 44100.0;
    float m_highPassState = 0.0f;
    float m_lowPassState = 0.0f;
};


} // namespace Aura::DSP::Analysis
