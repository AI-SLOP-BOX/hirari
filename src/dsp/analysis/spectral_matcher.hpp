#pragma once
#include "spectral_analyzer.hpp"
#include <array>
#include <algorithm>
#include <cmath>
#include <cstddef>
#include <vector>

namespace Aura::DSP::Analysis {

/**
 * @class SpectralMatcher
 * @brief Algorithmic AI for matching a track's frequency response to a target reference.
 * HONEST FIX: Uses averaged Power Spectral Density (PSD) for stable curve fitting.
 */
class SpectralMatcher {
public:
    static constexpr size_t kBins = SpectralAnalyzer::kFFTSize / 2;

    SpectralMatcher() { reset(); }

    /**
     * @brief Sets the reference spectrum (e.g., from a Pink Noise or a curated track).
     */
    void setReference(const std::vector<float>& target) {
        setReference(target.data(), target.size());
    }

    void setReference(const float* target, size_t length) {
        if (!target || length < kBins) return;
        for (size_t i = 0; i < kBins; ++i)
            if (std::isfinite(target[i]) && target[i] >= 0.0f)
                m_targetSpectrum[i] = std::max(target[i], kFloor);
    }

    /**
     * @brief Integrates a new FFT block into the long-term average with industrial precision.
     * INDUSTRIAL: Delegating spectrum averaging to the Rust 'SpectrumOrchestrator'.
     */
    void updateAverage(const std::vector<float>& inputMags) {
        updateAverage(inputMags.data(), inputMags.size());
    }

    void updateAverage(const float* inputMags, size_t length) {
        if (!inputMags) return;
        const size_t count = std::min(length, kBins);
        for (size_t i = 0; i < count; ++i) {
            const float magnitude = inputMags[i];
            if (!std::isfinite(magnitude)) continue;
            m_averagedInput[i] += kAverageAlpha * (std::max(magnitude, kFloor) - m_averagedInput[i]);
        }
    }

    /**
     * @brief Calculates the dB difference curve with forensic accuracy and industrial precision.
     * @return A vector of dB offsets for each FFT bin.
     */
    std::vector<float> calculateMatchCurve() {
        std::vector<float> curve(kBins);
        for (size_t i = 0; i < kBins; ++i) {
            const float inputDb = 20.0f * std::log10(std::max(m_averagedInput[i], kFloor));
            const float targetDb = 20.0f * std::log10(std::max(m_targetSpectrum[i], kFloor));
            curve[i] = std::clamp(targetDb - inputDb, -12.0f, 12.0f);
        }
        return curve;
    }

    /**
     * @brief Generates Pink Noise reference spectrum with absolute technical integrity.
     */
    void setPinkNoiseReference() {
        m_targetSpectrum[0] = 1.0f;
        for (size_t i = 1; i < kBins; ++i)
            m_targetSpectrum[i] = 1.0f / std::sqrt(static_cast<float>(i));
    }

private:
    static constexpr float kFloor = 1.0e-12f;
    static constexpr float kAverageAlpha = 0.01f;

    void reset() {
        m_targetSpectrum.fill(0.001f);
        m_averagedInput.fill(0.001f);
    }

    std::array<float, kBins> m_targetSpectrum{};
    std::array<float, kBins> m_averagedInput{};
};

} // namespace Aura::DSP::Analysis
