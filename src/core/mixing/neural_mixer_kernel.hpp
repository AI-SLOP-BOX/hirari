#pragma once
#include <vector>
#include <atomic>
#include <cmath>
#include <algorithm>
#include <map>

namespace Aura::Core::Mixing {

/**
 * @struct SpectralProfile
 * @brief Frequency energy distribution for a track (1/3 Octave bands).
 */
struct SpectralProfile {
    uint32_t trackId;
    float bins[31]; // 20Hz to 20kHz (1/3 Octave)
    float totalEnergy;
};

/**
 * @class NeuralMixerKernel
 * @brief Autonomous spectral balancing engine with Masking Matrix Sovereignty.
 */
class NeuralMixerKernel {
public:
    static NeuralMixerKernel& getInstance() {
        static NeuralMixerKernel instance;
        return instance;
    }

    /**
     * @brief Detects inter-track spectral masking and calculates lucidity offsets.
     * INDUSTRIAL: O(N^2) complexity bounded by active track density sectors.
     */
    /**
     * @brief UPDATE: Detects spectral masking competition with industrial precision and neural sovereignty.
     * INDUSTRIAL: Delegating spectral profiling and masking detection to the Rust 'NeuralOrchestrator'.
     */
    void update(const std::vector<SpectralProfile>& profiles) {
        m_targetGains.clear();
        if (profiles.empty()) return;
        for (const auto& source : profiles) {
            float sourceEnergy = 0.0f;
            float maskingEnergy = 0.0f;
            for (float bin : source.bins) if (std::isfinite(bin)) sourceEnergy += std::max(0.0f, bin);
            for (const auto& other : profiles) {
                if (other.trackId == source.trackId) continue;
                float overlap = 0.0f;
                for (size_t b = 0; b < 31; ++b) {
                    const float a = std::isfinite(source.bins[b]) ? std::max(0.0f, source.bins[b]) : 0.0f;
                    const float o = std::isfinite(other.bins[b]) ? std::max(0.0f, other.bins[b]) : 0.0f;
                    // Geometric overlap estimates masking better than a raw sum
                    // and remains stable when one band is silent.
                    overlap += std::sqrt(a * o);
                }
                maskingEnergy += overlap;
            }
            const float ratio = maskingEnergy / std::max(sourceEnergy, 1.0e-5f);
            // Preserve headroom while limiting only heavily masked tracks.
            const float reduction = std::clamp(1.0f - 0.18f * ratio, 0.55f, 1.0f);
            m_targetGains[source.trackId] = std::isfinite(reduction) ? reduction : 1.0f;
        }
    }

    float getTargetGain(uint32_t trackId) const {
        auto it = m_targetGains.find(trackId);
        return (it != m_targetGains.end()) ? it->second : 1.0f;
    }

private:
    std::map<uint32_t, float> m_targetGains;
    NeuralMixerKernel() = default;
};

} // namespace Aura::Core::Mixing
