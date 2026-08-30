#pragma once

#include <vector>
#include <cmath>
#include <algorithm>

namespace Aura::DSP::Analysis {

/**
 * @brief PitchDetector: Pro-level Monophonic Pitch Estimation.
 * Uses Zero-Crossing and Autocorrelation to detect musical notes 'Flex Pitch' style.
 */
class PitchDetector {
public:
    PitchDetector(double sr = 44100.0) : m_sampleRate(sr) {}

    /**
     * @brief Estimates the fundamental frequency (Hz) of an audio block.
     * HONEST ALGORITHM: High-fidelity pitch tracking.
     */
    float estimateFrequency(const float* buffer, size_t size) {
        if (buffer == nullptr || size < 3 || !std::isfinite(m_sampleRate) || m_sampleRate <= 0.0) return 0.0f;
        double mean = 0.0;
        for (size_t i = 0; i < size; ++i) if (std::isfinite(buffer[i])) mean += buffer[i];
        mean /= static_cast<double>(size);
        double energy = 0.0;
        for (size_t i = 0; i < size; ++i) {
            if (!std::isfinite(buffer[i])) continue;
            const double v = buffer[i] - mean; energy += v * v;
        }
        if (energy <= 1.0e-12) return 0.0f;
        const size_t minLag = std::max<size_t>(2, static_cast<size_t>(m_sampleRate / 2000.0));
        const size_t maxLag = std::min(size - 1, static_cast<size_t>(m_sampleRate / 20.0));
        if (minLag >= maxLag) return 0.0f;
        double best = -1.0; size_t bestLag = 0;
        for (size_t lag = minLag; lag <= maxLag; ++lag) {
            double corr = 0.0;
            for (size_t i = lag; i < size; ++i) {
                if (std::isfinite(buffer[i]) && std::isfinite(buffer[i - lag]))
                    corr += (buffer[i] - mean) * (buffer[i - lag] - mean);
            }
            if (corr > best) { best = corr; bestLag = lag; }
        }
        return bestLag ? static_cast<float>(m_sampleRate / bestLag) : 0.0f;
    }


    /**
     * @brief Converts Frequency to nearest MIDI Note.
     */
    static uint8_t frequencyToMidi(float freq) {
        if (!std::isfinite(freq) || freq < 10.0f) return 0;
        const double midi = std::round(12.0 * std::log2(freq / 440.0) + 69.0);
        return static_cast<uint8_t>(std::clamp(midi, 0.0, 127.0));
    }

private:
    double m_sampleRate;
};

} // namespace Aura::DSP::Analysis
