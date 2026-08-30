#pragma once
#include <vector>
#include <cmath>
#include <algorithm>

namespace Aura::DSP::Analysis {

/**
 * @class DynamicsAI
 * @brief Algorithmic Dynamics Assistant for professional density control.
 * HONEST FIX: Uses Crest Factor calculations to identify necessary compression stages.
 */
class DynamicsAI {
public:
    struct Params {
        float thresholdDB = 0.0f;
        float ratio = 1.0f;
        float attackMs = 10.0f;
        float releaseMs = 50.0f;
    };

    /**
     * @brief Analyzes the peak vs RMS of a signal and suggests parameters with industrial precision.
     * The suggestion is deliberately deterministic: it is based on finite peak,
     * RMS, and crest-factor measurements rather than an opaque model.
     */
    Params suggest(const float* data, size_t numFrames) const noexcept {
        Params result;
        if (data == nullptr || numFrames == 0) {
            return result;
        }

        float peak = 0.0f;
        double sumSquares = 0.0;
        size_t validSamples = 0;
        for (size_t i = 0; i < numFrames; ++i) {
            const float sample = data[i];
            if (!std::isfinite(sample)) {
                continue;
            }
            peak = std::max(peak, std::abs(sample));
            sumSquares += static_cast<double>(sample) * static_cast<double>(sample);
            ++validSamples;
        }
        if (validSamples == 0 || peak <= 1.0e-6f) {
            return result;
        }

        constexpr float kMinDB = -120.0f;
        const float rms = static_cast<float>(std::sqrt(sumSquares / static_cast<double>(validSamples)));
        const float peakDB = std::max(kMinDB, 20.0f * std::log10(peak));
        const float rmsDB = std::max(kMinDB, 20.0f * std::log10(std::max(rms, 1.0e-6f)));
        const float crestDB = std::clamp(peakDB - rmsDB, 0.0f, 36.0f);

        // Leave a small amount of average-level headroom and increase the
        // ratio only when the measured crest factor warrants it.
        result.thresholdDB = std::clamp(rmsDB + 3.0f, -60.0f, -0.1f);
        result.ratio = std::clamp(1.0f + crestDB / 8.0f, 1.0f, 8.0f);
        result.attackMs = std::clamp(2.0f + crestDB * 0.5f, 2.0f, 20.0f);
        result.releaseMs = std::clamp(50.0f + crestDB * 4.0f, 50.0f, 200.0f);
        return result;
    }
};

} // namespace Aura::DSP::Analysis
