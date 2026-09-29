#pragma once

#include <atomic>

namespace Hirari::Core::DSP::Mixing {

/**
 * @brief MSProcessor: Mid-Side encoding and decoding for mastering imaging.
 * Iconic Logic Pro feature for widening side signals while maintaining mono-punch.
 */
class MSProcessor {
public:
    void process(float* l, float* r, size_t numFrames) {
        float targetMid = m_midGain.load();
        float targetSide = m_sideGain.load();
        
        // Smoothing step calculation
        float midStep = (targetMid - m_currentMid) / static_cast<float>(numFrames);
        float sideStep = (targetSide - m_currentSide) / static_cast<float>(numFrames);

        for (size_t i = 0; i < numFrames; ++i) {
            // ENCODE to Mid/Side
            float mid = (l[i] + r[i]) * 0.5f;
            float side = (l[i] - r[i]) * 0.5f;

            // PROCESS (Gain balancing with smoothing)
            m_currentMid += midStep;
            m_currentSide += sideStep;
            
            mid *= m_currentMid;
            side *= m_currentSide;

            // DECODE back to L/R
            l[i] = mid + side;
            r[i] = mid - side;
        }
    }

    void setMidGain(float g) { m_midGain.store(g); }
    void setSideGain(float g) { m_sideGain.store(g); }

private:
    std::atomic<float> m_midGain{1.0f}, m_sideGain{1.0f};
    float m_currentMid = 1.0f, m_currentSide = 1.0f;
};

} // namespace Hirari::Core::DSP::Mixing
