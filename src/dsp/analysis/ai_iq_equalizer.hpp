#pragma once

#include <vector>
#include <string>
#include <map>
#include <atomic>
#include <array>
#include <cmath>
#include "spectrum_analyzer.hpp"

namespace Aura::DSP::Analysis {

/**
 * @brief AI_IQ_Equalizer: Intelligent spectral balance advisor.
 * Analyzes frequency clashing between tracks and suggests surgical EQ cuts.
 */
class AI_IQ_Equalizer {
public:
    static AI_IQ_Equalizer& getInstance() {
        static AI_IQ_Equalizer instance;
        return instance;
    }

    struct EQ_Suggestion {
        float frequencyHz;
        float gainDB;
        float qFactor;
        std::string comment;
    };

    /**
     * @brief Analyzes clashing frequencies between a target track and its references.
     */
    EQ_Suggestion suggestCorrection(uint32_t trackId, uint32_t referenceTrackId) {
        const auto a = m_profiles.find(trackId), b = m_profiles.find(referenceTrackId);
        if (a == m_profiles.end() || b == m_profiles.end()) return {0.0f, 0.0f, 0.0f, "No spectral profile available."};
        float peak = 0.0f, total = 0.0f;
        size_t peakBin = 0;
        for (size_t i = 0; i < 31; ++i) {
            const float av = std::max(0.0f, a->second[i]);
            const float bv = std::max(0.0f, b->second[i]);
            total += std::min(av, bv);
            if (std::min(av, bv) > peak) { peak = std::min(av, bv); peakBin = i; }
        }
        if (peak <= 1.0e-6f) return {0.0f, 0.0f, 0.0f, "No significant spectral clash."};
        const float frequency = 20.0f * std::pow(2.0f, static_cast<float>(peakBin) / 3.0f);
        const float reduction = -std::clamp(3.0f + 9.0f * peak / std::max(total, 1.0e-6f), 3.0f, 12.0f);
        return {frequency, reduction, 1.4f, "Surgical cut to reduce spectral masking."};
    }

    bool setTrackProfile(uint32_t trackId, const float* bins, size_t count) {
        if (!bins || count == 0) return false;
        std::array<float, 31> profile{};
        for (size_t i = 0; i < std::min<size_t>(31, count); ++i)
            profile[i] = std::isfinite(bins[i]) ? std::max(0.0f, bins[i]) : 0.0f;
        m_profiles[trackId] = profile;
        return true;
    }

    /**
     * @brief Returns a global spectral health check.
     */
    std::string getHealthReport() {
        if (m_profiles.empty()) return "Spectral Balance: no track profiles available.";
        double low = 0.0, high = 0.0;
        for (const auto& [id, profile] : m_profiles) {
            (void)id;
            for (size_t i = 0; i < 31; ++i) {
                if (i < 10) low += profile[i];
                else if (i > 20) high += profile[i];
            }
        }
        const double ratio = high / std::max(low, 1.0e-6);
        if (ratio < 0.25) return "Spectral Balance: high-frequency clarity is low.";
        if (ratio > 2.5) return "Spectral Balance: high-frequency energy is dominant.";
        return "Spectral Balance: low/high energy is within professional limits.";
    }

private:
    AI_IQ_Equalizer() = default;
    std::map<uint32_t, std::array<float, 31>> m_profiles;
};

} // namespace Aura::DSP::Analysis
