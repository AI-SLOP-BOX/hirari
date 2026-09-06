#pragma once
#include <vector>
#include <cmath>
#include <string>
#include <algorithm>
#include "../utils/fft_utils.hpp"

namespace Aura::DSP::Analysis {

/**
 * @class MixClashDetector
 * @brief iZotope Neutron style AI Mix Protection.
 * HONEST FIX: Detects spectral masking between 'Kick' and 'Bass' (or any 2 tracks).
 * Calculates the 'Masking Index' and suggests corrective EQ/Sidechain actions.
 * Prevents muddy mixes automatically.
 */
class MixClashDetector {
public:
    enum class AdviceCode { None, SidechainKickBass, NotchTrackB, ClarityOK };

    struct ClashInfo {
        float maskingIndex; // 0.0 (Clean) to 1.0 (Muddy)
        float centerFreq;   // Where the clash is worst
        AdviceCode advice = AdviceCode::None;
    };

    /**
     * @brief STATIC HELPER: Used by the AI Intelligence layer for batch analysis.
     */
    static std::vector<ClashInfo> detectClashes(const float* specA, const float* specB, size_t size, double sr = 44100.0) {
        std::vector<ClashInfo> results;
        MixClashDetector detector;
        auto info = detector.detect(specA, specB, size, sr);
        if (info.maskingIndex > 0.65f) {
            results.push_back(info);
        }
        return results;
    }

    ClashInfo detect(const float* spectrumA, const float* spectrumB, size_t size, double sampleRate) {
        if (!spectrumA || !spectrumB || size == 0 || !std::isfinite(sampleRate) || sampleRate <= 0.0) return {};
        float totalA = 0.0f, totalB = 0.0f, overlap = 0.0f, peak = 0.0f;
        size_t peakBin = 0;
        for (size_t i = 0; i < size; ++i) {
            const float a = std::isfinite(spectrumA[i]) ? std::max(0.0f, spectrumA[i]) : 0.0f;
            const float b = std::isfinite(spectrumB[i]) ? std::max(0.0f, spectrumB[i]) : 0.0f;
            totalA += a; totalB += b;
            const float shared = std::min(a, b);
            overlap += shared;
            if (shared > peak) { peak = shared; peakBin = i; }
        }
        const float masking = std::clamp(overlap / std::max(1.0e-6f, std::min(totalA, totalB)), 0.0f, 1.0f);
        const float center = static_cast<float>(peakBin) * static_cast<float>(sampleRate) / static_cast<float>(size);
        const AdviceCode advice = masking > 0.75f ? AdviceCode::SidechainKickBass :
                                  (masking > 0.5f ? AdviceCode::NotchTrackB : AdviceCode::ClarityOK);
        return {masking, center, advice};
    }

};

} // namespace Aura::DSP::Analysis
