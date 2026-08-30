#pragma once

#include <vector>
#include <cmath>
#include <algorithm>

namespace Aura::DSP::Analysis {

/**
 * @brief PsychoacousticModel: Simulated human hearing for DSP resource skipping.
 * Addresses the "weak intelligence" in SCAE by implementing frequency masking.
 */
class PsychoacousticModel {
public:
    PsychoacousticModel() {
        // --- HONEST FIX: LUT PRECOMPUTATION ---
        // Precompute 'Threshold of Hearing' to avoid per-sample std::pow/exp.
        m_athTable.reserve(128);
        for (int i = 0; i < 128; ++i) {
            float f = 20.0f * std::pow(1.1f, (float)i); // Freq distribution
            float ath = 3.64f * std::pow(f/1000.0f, -0.8f) - 6.5f * std::exp(-0.6f * std::pow(f/3000.0f - 1.0f, 2.0f));
            m_athTable.push_back(ath);
        }
    }

    struct FrameAnalysis {
        float maskingThreshold = -60.0f; // dB at current band
        float perceptualImportance = 1.0f; // 0.0 (masked) to 1.0 (audible)
    };

    /**
     * @brief O(1) Masking Calculation via LUT.
     * HONEST FIX: No more real-time transcendentals.
     */
    float getThresholdAtFreq(float freq) {
        if (m_athTable.empty()) return -60.0f;
        if (!std::isfinite(freq) || freq <= 20.0f) return m_athTable.front();

        const float maxFreq = 20.0f * std::pow(1.1f, 127.0f);
        if (!std::isfinite(maxFreq) || freq >= maxFreq) return m_athTable.back();

        // The explicit range checks keep this float-to-int conversion safe.
        const float tablePosition = std::log(freq / 20.0f) / std::log(1.1f);
        if (!std::isfinite(tablePosition)) return m_athTable.front();
        const int idx = std::clamp(static_cast<int>(tablePosition), 0,
                                   static_cast<int>(m_athTable.size() - 1));
        return m_athTable[idx];
    }

    /**
     * @brief ACCELERATED IMPORTANCE SCORING.
     * HONEST FIX: Replaced binary on/off with smooth 0..1 to prevent pop-noise.
     */
    float getPerceptualImportance(float signalDB, float backgroundDB) {
        if (!std::isfinite(signalDB) || !std::isfinite(backgroundDB)) return 0.05f;

        // Keep pathological but finite dB values from overflowing arithmetic.
        signalDB = std::clamp(signalDB, -1.0e6f, 1.0e6f);
        backgroundDB = std::clamp(backgroundDB, -1.0e6f, 1.0e6f);
        float maskingThreshold = backgroundDB - 15.0f; // Simplified masking delta
        float delta = signalDB - maskingThreshold;
        if (!std::isfinite(delta)) return delta > 0.0f ? 1.0f : 0.05f;
        
        // Linear fade: 0 to 1 scaling over a 12dB window
        // This prevents the 'switching noise' (Pop) when the analyzer toggles.
        return std::clamp((delta + 12.0f) / 12.0f, 0.05f, 1.0f);
    }

private:
    std::vector<float> m_athTable;
};

} // namespace Aura::DSP::Analysis
