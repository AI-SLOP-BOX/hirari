#pragma once

#include <vector>
#include <cmath>
#include <atomic>

namespace Hirari::Core::DSP::Mixing {

/**
 * @brief SidechainCompressor: External signal dynamic range processor.
 * Iconic Logic Pro feature for "Pumping" effects and vocal/instrument ducking.
 */
class SidechainCompressor {
public:
    /**
     * @brief Processes the main audio using an external trigger energy.
     */
    void process(float* l, float* r, const float* sidechainInput, size_t numFrames) {
        float threshLinear = std::pow(10.0f, m_threshold.load() / 20.0f);
        float ratio = m_ratio.load();
        
        // Envelope coefficients (simplified - for real app use sample rate)
        float attack = 0.99f;
        float release = 0.999f;

        for (size_t i = 0; i < numFrames; ++i) {
            // DETECTOR Stage: Analyze external sidechain energy
            float scEnergy = std::abs(sidechainInput[i]);
            
            // Envelope Follower (Peak)
            if (scEnergy > m_envelope) m_envelope = m_envelope * attack + scEnergy * (1.0f - attack);
            else m_envelope = m_envelope * release + scEnergy * (1.0f - release);

            // GAIN Reducer: Apply compression only when sidechain exceeds threshold
            float attenuation = 1.0f;
            if (m_envelope > threshLinear) {
                // Professional Gain Reduction formula
                float overDb = 20.0f * std::log10(m_envelope / threshLinear);
                float reducedDb = overDb / ratio;
                attenuation = std::pow(10.0f, (reducedDb - overDb) / 20.0f);
            }

            l[i] *= attenuation;
            r[i] *= attenuation;
        }
    }

private:
    std::atomic<float> m_threshold{-20.0f}, m_ratio{4.0f};
    float m_envelope = 0;
};

} // namespace Hirari::Core::DSP::Mixing
