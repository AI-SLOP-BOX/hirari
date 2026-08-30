#include "analyzer_8khz.hpp"
#include <cmath>

namespace Aura::DSP::Analysis {

Analyzer8kHz::Analyzer8kHz(double sr) {
    setSampleRate(sr);
}

void Analyzer8kHz::setSampleRate(double sr) {
    m_sampleRate = (std::isfinite(sr) && sr > 0.0) ? sr : 44100.0;
    m_highPassState = 0.0f;
    m_lowPassState = 0.0f;
}

void Analyzer8kHz::analyze(const float* buffer, size_t numFrames) {
    if (buffer == nullptr || numFrames == 0) {
        m_rms.store(0.0f, std::memory_order_relaxed);
        m_peak.store(0.0f, std::memory_order_relaxed);
        m_highFreqEnergy.store(0.0f, std::memory_order_relaxed);
        return;
    }

    const float highPassCoeff = static_cast<float>(
        1.0 / (1.0 + (2.0 * M_PI * 6000.0 / m_sampleRate)));
    const float lowPassCoeff = static_cast<float>(
        1.0 - std::exp(-(2.0 * M_PI * 10000.0) / m_sampleRate));
    double sumSquares = 0.0;
    double bandSumSquares = 0.0;
    float peak = 0.0f;

    for (size_t i = 0; i < numFrames; ++i) {
        const float sample = buffer[i];
        if (!std::isfinite(sample)) continue;

        const float magnitude = std::fabs(sample);
        sumSquares += static_cast<double>(sample) * sample;
        peak = std::max(peak, magnitude);

        // High-pass followed by low-pass gives a small, allocation-free band
        // around 8 kHz while retaining state across input blocks.
        m_highPassState += highPassCoeff * (sample - m_highPassState);
        const float highPassed = sample - m_highPassState;
        m_lowPassState += lowPassCoeff * (highPassed - m_lowPassState);
        bandSumSquares += static_cast<double>(m_lowPassState) * m_lowPassState;
    }

    const float rms = static_cast<float>(std::sqrt(sumSquares / numFrames));
    const float bandRms = static_cast<float>(std::sqrt(bandSumSquares / numFrames));
    m_rms.store(std::isfinite(rms) ? rms : 0.0f, std::memory_order_relaxed);
    m_peak.store(std::isfinite(peak) ? peak : 0.0f, std::memory_order_relaxed);
    m_highFreqEnergy.store(std::isfinite(bandRms) ? bandRms : 0.0f,
                           std::memory_order_relaxed);
}

float Analyzer8kHz::getEnergy() const {
    return m_highFreqEnergy.load(std::memory_order_relaxed);
}

} // namespace Aura::DSP::Analysis
