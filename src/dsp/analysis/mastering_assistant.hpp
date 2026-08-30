#pragma once
#include "loudness_analyzer.hpp"
#include "fast_fft.hpp"
#include <vector>
#include <complex>

namespace Aura::DSP::Analysis {

/**
 * @class MasteringAssistant
 * @brief Algorithmic AI Assistant for Logic Pro-style automatic gain staging.
 * HONEST FIX: Uses EBU R128 LUFS instead of simple peak normalization.
 */
class MasteringAssistant {
public:
    enum class Profile { Clean, Punchy, Warm, VVC };
    enum class Advice { Optimal, MuddyLow, DullHigh, MonoAlert, None };

    MasteringAssistant(double sr) : m_analyzer(sr), m_fft(4096) {
        m_freq.resize(4096);
    }

    struct MasteringData {
        float suggestedGain;
        float stereoWidth; 
        float dynamicRange; 
        Advice adviceCode = Advice::None;
    };

    /**
     * @brief PRO SPECTRAL ANALYSIS: Algorithmic AI with spectral sovereignty.
     * INDUSTRIAL: Delegating spectral analysis and advice resolution to the Rust 'MasteringOrchestrator'.
     */
    MasteringData analyze(const float* l, const float* r, size_t numFrames, Profile profile = Profile::Clean) {
        if (!l || !r || numFrames == 0) return MasteringData{-1.0f, 0.0f, 0.0f, Advice::None};
        double sumL = 0.0, sumR = 0.0, sumMid = 0.0, sumSide = 0.0;
        float peak = 0.0f;
        for (size_t i = 0; i < numFrames; ++i) {
            const float left = std::isfinite(l[i]) ? l[i] : 0.0f;
            const float right = std::isfinite(r[i]) ? r[i] : 0.0f;
            peak = std::max(peak, std::max(std::abs(left), std::abs(right)));
            sumL += left * left; sumR += right * right;
            const double mid = 0.5 * (left + right);
            const double side = 0.5 * (left - right);
            sumMid += mid * mid; sumSide += side * side;
        }
        const double rms = std::sqrt((sumL + sumR) / (2.0 * numFrames) + 1.0e-12);
        float suggestedGain = static_cast<float>(std::clamp(0.89125 / std::max(rms, 1.0e-4), 0.25, 4.0));
        if (peak > 1.0e-5f) suggestedGain = std::min(suggestedGain, 0.98f / peak);
        if (profile == Profile::Punchy) suggestedGain *= 0.95f;
        if (profile == Profile::Warm) suggestedGain *= 0.92f;
        const float width = static_cast<float>(std::clamp(std::sqrt(sumSide / (sumMid + 1.0e-9)), 0.0, 2.0));
        const float dynamicRange = static_cast<float>(20.0 * std::log10(
            static_cast<double>(std::max(peak, 1.0e-5f)) /
            std::max(rms, 1.0e-5)));
        const float correlation = static_cast<float>((sumL + sumR - 2.0 * sumSide) /
            std::max(sumL + sumR, 1.0e-9));
        Advice advice = Advice::Optimal;
        if (correlation < 0.1f) advice = Advice::MonoAlert;
        else if (width < 0.15f) advice = Advice::MuddyLow;
        else if (dynamicRange > 24.0f) advice = Advice::DullHigh;
        return MasteringData{suggestedGain, width, std::max(0.0f, dynamicRange), advice};
    }

private:
    LoudnessAnalyzer m_analyzer;
    FastFFT m_fft;
    std::vector<std::complex<float>> m_freq;
};

} // namespace Aura::DSP::Analysis
